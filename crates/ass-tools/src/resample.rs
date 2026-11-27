//! Resolution resampling matching Aegisub 3.5.1 `Subtitle -> Resample Resolution`.
//!
//! Reference behaviour (Aegisub 3.5.1 `src/resolution_resampler.cpp`):
//! - Resolves source resolution from `[Script Info]` (`PlayResX`/`PlayResY`, defaulting to 640×480
//!   if missing/zero, or explicit `source_override`).
//! - Computes horizontal/vertical scale factors (`rx`, `ry`), aspect-ratio stretch `ar = rx / ry`,
//!   and coordinate offsets (`shift_x`, `shift_y`) according to `AspectRatioMode` and margin offsets:
//!   - `Stretch` (anamorphic): scales X by `rx`, Y by `ry`, font/borders/shadows by `ry`, and
//!     multiplies `ScaleX` and `\fscx` by `ar`.
//!   - `AddBorders` (letterbox/pillarbox): uniform scale `r = min(rx, ry)`, centering content by
//!     adding symmetric X or Y shift.
//!   - `RemoveBorders` (crop): uniform scale `r = max(rx, ry)`, cropping symmetrically.
//! - Scales `[Script Info]` (`PlayResX`, `PlayResY`, `LayoutResX`/`LayoutResY` if present, optional
//!   `YCbCr Matrix`), all `[V4+ Styles]` (fontsize, outline, shadow, spacing, `scale_x`, margins),
//!   and all event override tags (`\pos`, `\move`, `\org`, `\clip`/`\iclip` rect & vector, `\fs`,
//!   `\fsp`, `\bord`, `\xbord`, `\ybord`, `\shad`, `\xshad`, `\yshad`, `\be`, `\blur`, `\fscx`,
//!   `\pbo`, including tags nested inside `\t(...)`) plus `\p>0` vector drawing coordinates.

use crate::ToolsError;
use ass_core::{scan_override_block, AssStyle, EditCommand, RawOverrideItem, ScriptFile};
use std::fmt::Write as _;

/// Aspect-ratio handling mode matching Aegisub 3.5.1's Resample Resolution dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AspectRatioMode {
    /// Anamorphic stretch: independent X/Y scaling, adjusting `ScaleX` and `\fscx` by `rx / ry`.
    Stretch,
    /// Keep aspect ratio by letterboxing/pillarboxing (`r = min(rx, ry)` + centering offset).
    #[default]
    AddBorders,
    /// Keep aspect ratio by cropping (`r = max(rx, ry)` + negative centering offset).
    RemoveBorders,
}

/// Configuration for [`resample_resolution`].
#[derive(Debug, Clone)]
pub struct ResampleOptions {
    pub dest_x: u32,
    pub dest_y: u32,
    pub source_override: Option<(u32, u32)>,
    pub ar_mode: AspectRatioMode,
    pub margin_left: i32,
    pub margin_right: i32,
    pub margin_top: i32,
    pub margin_bottom: i32,
    /// Optional new `YCbCr Matrix` header value (e.g. `"TV.709"`).
    pub ycbcr_matrix: Option<String>,
}

impl ResampleOptions {
    /// Creates standard keep-aspect (`AddBorders`) resample options to `dest_x × dest_y`.
    #[must_use]
    pub const fn new(dest_x: u32, dest_y: u32) -> Self {
        Self {
            dest_x,
            dest_y,
            source_override: None,
            ar_mode: AspectRatioMode::AddBorders,
            margin_left: 0,
            margin_right: 0,
            margin_top: 0,
            margin_bottom: 0,
            ycbcr_matrix: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResampleState {
    rx: f64,
    ry: f64,
    ar: f64,
    shift_x: f64,
    shift_right: f64,
    shift_y: f64,
    color_conv: Option<YcbcrConverter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum YcbcrMatrix {
    Bt601,
    Bt709,
    Fcc,
    Smpte240m,
}

impl YcbcrMatrix {
    const fn coefficients(self) -> [f64; 3] {
        match self {
            Self::Bt601 => [0.299, 0.587, 0.114],
            Self::Bt709 => [0.2126, 0.7152, 0.0722],
            Self::Fcc => [0.3, 0.59, 0.11],
            Self::Smpte240m => [0.212, 0.701, 0.087],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum YcbcrRange {
    Tv,
    Pc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct YcbcrColorspace {
    matrix: YcbcrMatrix,
    range: YcbcrRange,
}

/// Linear + offset RGB->YCbCr->RGB converter matching Aegisub's `agi::ycbcr_converter`.
#[derive(Debug, Clone, Copy)]
struct YcbcrConverter {
    to_ycbcr: [f64; 9],
    from_ycbcr: [f64; 9],
    shift_to: [f64; 3],
    shift_from: [f64; 3],
}

impl YcbcrConverter {
    fn new(src: YcbcrColorspace, dst: YcbcrColorspace) -> Self {
        let [src_red, src_green, src_blue] = src.matrix.coefficients();
        let mut to_ycbcr = [
            src_red,
            src_green,
            src_blue,
            -src_red / (1.0 - src_blue),
            -src_green / (1.0 - src_blue),
            1.0,
            1.0,
            -src_green / (1.0 - src_red),
            -src_blue / (1.0 - src_red),
        ];
        let (row_scale, shift_to) = match src.range {
            YcbcrRange::Pc => ([1.0, 0.5, 0.5], [0.0, 128.0, 128.0]),
            YcbcrRange::Tv => (
                [219.0 / 255.0, 112.0 / 255.0, 112.0 / 255.0],
                [16.0, 128.0, 128.0],
            ),
        };
        for row in 0..3 {
            for col in 0..3 {
                to_ycbcr[row * 3 + col] *= row_scale[row];
            }
        }

        let [dst_red, dst_green, dst_blue] = dst.matrix.coefficients();
        let mut from_ycbcr = [
            1.0,
            0.0,
            1.0 - dst_red,
            1.0,
            -(1.0 - dst_blue) * dst_blue / dst_green,
            -(1.0 - dst_red) * dst_red / dst_green,
            1.0,
            1.0 - dst_blue,
            0.0,
        ];
        let (col_scale, shift_from) = match dst.range {
            YcbcrRange::Pc => ([1.0, 2.0, 2.0], [0.0, -128.0, -128.0]),
            YcbcrRange::Tv => (
                [255.0 / 219.0, 255.0 / 112.0, 255.0 / 112.0],
                [-16.0, -128.0, -128.0],
            ),
        };
        for row in 0..3 {
            for col in 0..3 {
                from_ycbcr[row * 3 + col] *= col_scale[col];
            }
        }

        Self {
            to_ycbcr,
            from_ycbcr,
            shift_to,
            shift_from,
        }
    }

    fn rgb_to_rgb(self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let input = [f64::from(r), f64::from(g), f64::from(b)];
        let ycbcr = [
            self.to_ycbcr[0] * input[0]
                + self.to_ycbcr[1] * input[1]
                + self.to_ycbcr[2] * input[2]
                + self.shift_to[0]
                + self.shift_from[0],
            self.to_ycbcr[3] * input[0]
                + self.to_ycbcr[4] * input[1]
                + self.to_ycbcr[5] * input[2]
                + self.shift_to[1]
                + self.shift_from[1],
            self.to_ycbcr[6] * input[0]
                + self.to_ycbcr[7] * input[1]
                + self.to_ycbcr[8] * input[2]
                + self.shift_to[2]
                + self.shift_from[2],
        ];
        let rgb = [
            self.from_ycbcr[0] * ycbcr[0]
                + self.from_ycbcr[1] * ycbcr[1]
                + self.from_ycbcr[2] * ycbcr[2],
            self.from_ycbcr[3] * ycbcr[0]
                + self.from_ycbcr[4] * ycbcr[1]
                + self.from_ycbcr[5] * ycbcr[2],
            self.from_ycbcr[6] * ycbcr[0]
                + self.from_ycbcr[7] * ycbcr[1]
                + self.from_ycbcr[8] * ycbcr[2],
        ];
        (clamp_u8(rgb[0]), clamp_u8(rgb[1]), clamp_u8(rgb[2]))
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_u8(v: f64) -> u8 {
    // Matches Aegisub `clamp(v + 0.5)` into [0, 255].
    let shifted = (v + 0.5).clamp(0.0, 255.0);
    shifted as u8
}

fn parse_ycbcr_colorspace(s: &str) -> Option<YcbcrColorspace> {
    let trimmed = s.trim();
    if trimmed.eq_ignore_ascii_case("TV.601") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Bt601,
            range: YcbcrRange::Tv,
        })
    } else if trimmed.eq_ignore_ascii_case("PC.601") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Bt601,
            range: YcbcrRange::Pc,
        })
    } else if trimmed.eq_ignore_ascii_case("TV.709") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Bt709,
            range: YcbcrRange::Tv,
        })
    } else if trimmed.eq_ignore_ascii_case("PC.709") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Bt709,
            range: YcbcrRange::Pc,
        })
    } else if trimmed.eq_ignore_ascii_case("TV.FCC") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Fcc,
            range: YcbcrRange::Tv,
        })
    } else if trimmed.eq_ignore_ascii_case("PC.FCC") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Fcc,
            range: YcbcrRange::Pc,
        })
    } else if trimmed.eq_ignore_ascii_case("TV.240M") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Smpte240m,
            range: YcbcrRange::Tv,
        })
    } else if trimmed.eq_ignore_ascii_case("PC.240M") {
        Some(YcbcrColorspace {
            matrix: YcbcrMatrix::Smpte240m,
            range: YcbcrRange::Pc,
        })
    } else {
        None
    }
}

fn resolve_color_converter(script: &ScriptFile, opts: &ResampleOptions) -> Option<YcbcrConverter> {
    let dst = parse_ycbcr_colorspace(opts.ycbcr_matrix.as_deref()?)?;
    let existing = script.script_info().and_then(|s| s.get("YCbCr Matrix"));
    let src = match existing.map(str::trim) {
        None | Some("") => YcbcrColorspace {
            // Matches Aegisub's Header::to_effective() default on missing header.
            matrix: YcbcrMatrix::Bt601,
            range: YcbcrRange::Tv,
        },
        Some(s) if s.eq_ignore_ascii_case("None") => return None,
        Some(s) => parse_ycbcr_colorspace(s)?,
    };
    if src == dst {
        None
    } else {
        Some(YcbcrConverter::new(src, dst))
    }
}

/// Resamples `script` to the target resolution in `opts`.
///
/// Returns one atomic [`EditCommand::Batch`] updating `[Script Info]` (`PlayResX`, `PlayResY`,
/// `LayoutResX`/`LayoutResY` when present, and `YCbCr Matrix` when requested), all `[V4+ Styles]`,
/// and all `[Events]`. Does not mutate `script` until the returned command is executed.
pub fn resample_resolution(
    script: &ScriptFile,
    opts: &ResampleOptions,
) -> Result<EditCommand, ToolsError> {
    if opts.dest_x == 0 || opts.dest_y == 0 {
        return Err(ToolsError::InvalidResolution(opts.dest_x, opts.dest_y));
    }

    let (src_x, src_y) = resolve_source_res(script, opts.source_override)?;
    let mut state = compute_state(src_x, src_y, opts);
    state.color_conv = resolve_color_converter(script, opts);

    let ev_cap = script.events_section().map_or(0, |s| s.entries.len());
    let st_cap = script.styles_section().map_or(0, |s| s.entries.len());
    let mut cmds = Vec::with_capacity(ev_cap + st_cap + 6);
    let width_str = opts.dest_x.to_string();
    let height_str = opts.dest_y.to_string();
    let info = script.script_info();

    let push_info = |cmds: &mut Vec<EditCommand>, key: &str, next: String| {
        let prev = info.and_then(|s| s.get(key)).map(str::to_owned);
        if prev.as_deref() != Some(next.as_str()) {
            cmds.push(EditCommand::SetScriptInfo {
                key: key.to_owned(),
                before: prev,
                after: Some(next),
            });
        }
    };

    push_info(&mut cmds, "PlayResX", width_str.clone());
    push_info(&mut cmds, "PlayResY", height_str.clone());
    if info.and_then(|s| s.get("LayoutResX")).is_some() {
        push_info(&mut cmds, "LayoutResX", width_str);
    }
    if info.and_then(|s| s.get("LayoutResY")).is_some() {
        push_info(&mut cmds, "LayoutResY", height_str);
    }
    if let Some(ref matrix) = opts.ycbcr_matrix {
        push_info(&mut cmds, "YCbCr Matrix", matrix.clone());
    }

    // Scale all styles.
    if let Some(styles_sec) = script.styles_section() {
        for (idx, st) in styles_sec.styles().enumerate() {
            let after = resample_style(st, state);
            cmds.push(EditCommand::ModifyStyle {
                index: idx,
                before: Box::new(st.clone()),
                after: Box::new(after),
            });
        }
    }

    // Scale all events (margins + override tags + drawings).
    // Note (Aegisub resolution_resampler.cpp): non-zero per-line margins receive the margin/border
    // shift just like styles, whereas 0 means "use style margin" and stays 0.
    if let Some(events_sec) = script.events_section() {
        for (idx, ev) in events_sec.events().enumerate() {
            let margin_left = if ev.margin_l() == 0 {
                0
            } else {
                scale_margin(ev.margin_l(), state.rx, state.shift_x)
            };
            let margin_right = if ev.margin_r() == 0 {
                0
            } else {
                scale_margin(ev.margin_r(), state.rx, state.shift_right)
            };
            let margin_vert = if ev.margin_v() == 0 {
                0
            } else {
                scale_margin(ev.margin_v(), state.ry, state.shift_y)
            };

            // Fast path: skip tag parser when text contains no `{` override block.
            let new_text = if ev.text().contains('{') {
                resample_event_text(ev.text(), state)
            } else {
                None
            };

            let margins_changed = margin_left != ev.margin_l()
                || margin_right != ev.margin_r()
                || margin_vert != ev.margin_v();

            if margins_changed {
                let mut after = ev.clone();
                after.set_margin_l(margin_left);
                after.set_margin_r(margin_right);
                after.set_margin_v(margin_vert);
                if let Some(t) = new_text {
                    after.set_text(t);
                }
                cmds.push(EditCommand::ModifyEvent {
                    index: idx,
                    before: Box::new(ev.clone()),
                    after: Box::new(after),
                });
            } else if let Some(t) = new_text {
                if t != ev.text() {
                    cmds.push(EditCommand::ModifyEventText {
                        index: idx,
                        before: ev.text().to_owned(),
                        after: t,
                    });
                }
            }
        }
    }

    Ok(EditCommand::Batch(cmds))
}

/// Resolves the source resolution. A *missing* `PlayRes` key falls back to Aegisub's 640x480
/// default; a key that is present but invalid is an error (never silently replaced).
fn resolve_source_res(
    script: &ScriptFile,
    override_res: Option<(u32, u32)>,
) -> Result<(u32, u32), ToolsError> {
    if let Some((x, y)) = override_res {
        if x > 0 && y > 0 {
            return Ok((x, y));
        }
    }
    let read = |key: &'static str, default: u32| -> Result<u32, ToolsError> {
        match script.script_info().and_then(|s| s.get(key)) {
            None => Ok(default),
            Some(v) => match v.trim().parse::<u32>() {
                Ok(n) if n > 0 => Ok(n),
                _ => Err(ToolsError::InvalidSourceResolution {
                    key,
                    value: v.to_owned(),
                }),
            },
        }
    };
    Ok((read("PlayResX", 640)?, read("PlayResY", 480)?))
}

fn compute_state(src_x: u32, src_y: u32, opts: &ResampleOptions) -> ResampleState {
    let eff_src_x =
        (f64::from(src_x) + f64::from(opts.margin_left) + f64::from(opts.margin_right)).max(1.0);
    let eff_src_y =
        (f64::from(src_y) + f64::from(opts.margin_top) + f64::from(opts.margin_bottom)).max(1.0);
    let scale_x = f64::from(opts.dest_x) / eff_src_x;
    let scale_y = f64::from(opts.dest_y) / eff_src_y;

    let (rx, ry, ar, border_x, border_y) = match opts.ar_mode {
        AspectRatioMode::Stretch => (scale_x, scale_y, scale_x / scale_y, 0.0, 0.0),
        AspectRatioMode::AddBorders => {
            let r = scale_x.min(scale_y);
            let bx = (f64::from(opts.dest_x) - eff_src_x * r) * 0.5;
            let by = (f64::from(opts.dest_y) - eff_src_y * r) * 0.5;
            (r, r, 1.0, bx, by)
        }
        AspectRatioMode::RemoveBorders => {
            let r = scale_x.max(scale_y);
            let bx = (f64::from(opts.dest_x) - eff_src_x * r) * 0.5;
            let by = (f64::from(opts.dest_y) - eff_src_y * r) * 0.5;
            (r, r, 1.0, bx, by)
        }
    };

    ResampleState {
        rx,
        ry,
        ar,
        shift_x: f64::from(opts.margin_left) * rx + border_x,
        shift_right: f64::from(opts.margin_right) * rx + border_x,
        shift_y: f64::from(opts.margin_top) * ry + border_y,
        color_conv: None,
    }
}

fn resample_style(st: &AssStyle, s: ResampleState) -> AssStyle {
    let mut out = st.clone();
    out.set_fontsize(round_2(st.fontsize() * s.ry));
    out.set_outline(round_2(st.outline() * s.ry));
    out.set_shadow(round_2(st.shadow() * s.ry));
    out.set_spacing(round_2(st.spacing() * s.rx));
    if (s.ar - 1.0).abs() > 1e-6 {
        out.set_scale_x(round_2(st.scale_x() * s.ar));
    }
    out.set_margin_l(scale_margin(st.margin_l(), s.rx, s.shift_x));
    out.set_margin_r(scale_margin(st.margin_r(), s.rx, s.shift_right));
    out.set_margin_v(scale_margin(st.margin_v(), s.ry, s.shift_y));
    if let Some(conv) = s.color_conv {
        if let Some(c) = convert_ass_style_colour(st.primary_colour(), conv) {
            out.set_primary_colour(c);
        }
        if let Some(c) = convert_ass_style_colour(st.secondary_colour(), conv) {
            out.set_secondary_colour(c);
        }
        if let Some(c) = convert_ass_style_colour(st.outline_colour(), conv) {
            out.set_outline_colour(c);
        }
        if let Some(c) = convert_ass_style_colour(st.back_colour(), conv) {
            out.set_back_colour(c);
        }
    }
    out
}

fn scale_margin(val: i32, factor: f64, shift: f64) -> i32 {
    if val == 0 && shift == 0.0 {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let res = (f64::from(val) * factor + shift).round() as i32;
    res.max(0)
}

fn resample_event_text(text: &str, state: ResampleState) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 32);
    let mut changed = false;
    let mut drawing_level = 0u32;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'{' {
            out.push('{');
            let (next, closed) = scan_override_block(text, i + 1, |item| match item {
                RawOverrideItem::Comment(c) => out.push_str(c),
                RawOverrideItem::Tag { name, raw_args } => {
                    if name == "p" {
                        if let Ok(lvl) = raw_args.trim().parse::<u32>() {
                            drawing_level = lvl;
                        }
                    }
                    write_resampled_tag(&mut out, name, raw_args, state, &mut changed, 0);
                }
            });
            if closed {
                out.push('}');
            }
            i = next;
        } else {
            let end = bytes[i..]
                .iter()
                .position(|&b| b == b'{')
                .map_or(bytes.len(), |rel| i + rel);
            let segment = &text[i..end];
            if drawing_level > 0 {
                // Aegisub resolution_resampler.cpp passes shift=(0, 0) and rx/ar for \p>0 drawings:
                // drawings are positioned by \pos or alignment/margins (not screen borders), and
                // dividing X by ar avoids double-stretching when Stretch scales ScaleX/scx by ar.
                let drawing_rx = state.rx / state.ar;
                let before = out.len();
                scale_drawing_commands(
                    &mut out,
                    segment,
                    drawing_rx,
                    state.ry,
                    0.0,
                    0.0,
                    drawing_level,
                );
                if out[before..] != *segment {
                    changed = true;
                }
            } else {
                out.push_str(segment);
            }
            i = end;
        }
    }

    changed.then_some(out)
}

fn write_resampled_tag(
    out: &mut String,
    name: &str,
    raw_args: &str,
    s: ResampleState,
    changed: &mut bool,
    depth: u8,
) {
    out.push('\\');
    out.push_str(name);

    match name {
        "pos" | "org" => {
            if let Some((x, y)) = parse_paren_f64_2(raw_args) {
                *changed = true;
                out.push('(');
                write_num(out, x * s.rx + s.shift_x);
                out.push(',');
                write_num(out, y * s.ry + s.shift_y);
                out.push(')');
                return;
            }
        }
        "move" => {
            if let Some((vals, count)) = parse_paren_f64_up_to_6(raw_args) {
                if count == 4 || count == 6 {
                    *changed = true;
                    out.push('(');
                    write_num(out, vals[0] * s.rx + s.shift_x);
                    out.push(',');
                    write_num(out, vals[1] * s.ry + s.shift_y);
                    out.push(',');
                    write_num(out, vals[2] * s.rx + s.shift_x);
                    out.push(',');
                    write_num(out, vals[3] * s.ry + s.shift_y);
                    if count == 6 {
                        #[allow(clippy::cast_possible_truncation)]
                        let a = vals[4].round() as i64;
                        #[allow(clippy::cast_possible_truncation)]
                        let b = vals[5].round() as i64;
                        out.push(',');
                        write_i64_ascii(out, a);
                        out.push(',');
                        write_i64_ascii(out, b);
                    }
                    out.push(')');
                    return;
                }
            }
        }
        "clip" | "iclip" => {
            if write_resampled_clip(out, raw_args, s, changed) {
                return;
            }
        }
        "t" if depth < 16 => {
            if let Some(inner) = strip_outer_parens(raw_args) {
                write_resampled_transform(out, raw_args, inner, s, changed, depth);
                return;
            }
        }
        "c" | "1c" | "2c" | "3c" | "4c" => {
            if let Some(conv) = s.color_conv {
                if write_converted_override_colour(out, raw_args, conv, changed) {
                    return;
                }
            }
        }
        "fs" | "bord" | "ybord" | "shad" | "yshad" | "be" | "blur" | "pbo" => {
            if write_scaled_scalar(out, raw_args, s.ry, changed) {
                return;
            }
        }
        "fsp" | "xbord" | "xshad" => {
            if write_scaled_scalar(out, raw_args, s.rx, changed) {
                return;
            }
        }
        "fscx"
            if (s.ar - 1.0).abs() > 1e-6 && write_scaled_scalar(out, raw_args, s.ar, changed) =>
        {
            return;
        }
        _ => {}
    }

    out.push_str(raw_args);
}

fn write_resampled_clip(
    out: &mut String,
    raw_args: &str,
    s: ResampleState,
    changed: &mut bool,
) -> bool {
    let Some(inner) = strip_outer_parens(raw_args) else {
        return false;
    };
    *changed = true;
    out.push('(');
    if let Some([x1, y1, x2, y2]) = parse_csv_f64_4(inner) {
        write_num(out, x1 * s.rx + s.shift_x);
        out.push(',');
        write_num(out, y1 * s.ry + s.shift_y);
        out.push(',');
        write_num(out, x2 * s.rx + s.shift_x);
        out.push(',');
        write_num(out, y2 * s.ry + s.shift_y);
    } else {
        let (scale, cmds) = inner
            .split_once(',')
            .and_then(|(sc, cmds)| sc.trim().parse::<u32>().ok().map(|v| (v, cmds.trim())))
            .unwrap_or((1, inner.trim()));
        if scale != 1 {
            write_i64_ascii(out, i64::from(scale));
            out.push(',');
        }
        scale_drawing_commands(out, cmds, s.rx, s.ry, s.shift_x, s.shift_y, scale);
    }
    out.push(')');
    true
}

fn write_resampled_transform(
    out: &mut String,
    raw_args: &str,
    inner: &str,
    s: ResampleState,
    changed: &mut bool,
    depth: u8,
) {
    let mut parts = [""; 4];
    let mut count = 0usize;
    let mut depth_parens: u32 = 0;
    let mut start = 0usize;
    for (idx, b) in inner.bytes().enumerate() {
        match b {
            b'(' => depth_parens = depth_parens.saturating_add(1),
            b')' => depth_parens = depth_parens.saturating_sub(1),
            b',' if depth_parens == 0 => {
                if count < 4 {
                    parts[count] = &inner[start..idx];
                }
                count += 1;
                start = idx + 1;
            }
            _ => {}
        }
    }
    if count < 4 {
        parts[count] = &inner[start..];
    }
    count += 1;

    let (t1, t2, accel, tags_str) = match count {
        1 => (None, None, None, parts[0]),
        2 => (None, None, parts[0].trim().parse::<f64>().ok(), parts[1]),
        3 => (
            parts[0].trim().parse::<i64>().ok(),
            parts[1].trim().parse::<i64>().ok(),
            None,
            parts[2],
        ),
        _ => (
            parts[0].trim().parse::<i64>().ok(),
            parts[1].trim().parse::<i64>().ok(),
            parts[2].trim().parse::<f64>().ok(),
            parts[3],
        ),
    };

    let save_len = out.len();
    out.push('(');
    if let (Some(a), Some(b)) = (t1, t2) {
        write_i64_ascii(out, a);
        out.push(',');
        write_i64_ascii(out, b);
        out.push(',');
    }
    if let Some(acc) = accel {
        write_num(out, acc);
        out.push(',');
    }
    let mut inner_changed = false;
    scan_override_block(tags_str, 0, |item| match item {
        RawOverrideItem::Comment(c) => out.push_str(c),
        RawOverrideItem::Tag { name, raw_args } => {
            write_resampled_tag(out, name, raw_args, s, &mut inner_changed, depth + 1);
        }
    });
    if inner_changed {
        out.push(')');
        *changed = true;
    } else {
        out.truncate(save_len);
        out.push_str(raw_args);
    }
}

#[inline]
fn write_scaled_scalar(out: &mut String, raw: &str, factor: f64, changed: &mut bool) -> bool {
    let Ok(val) = raw.trim().parse::<f64>() else {
        return false;
    };
    if !val.is_finite() {
        return false;
    }
    *changed = true;
    write_num(out, val * factor);
    true
}

#[inline]
fn write_converted_override_colour(
    out: &mut String,
    raw: &str,
    conv: YcbcrConverter,
    changed: &mut bool,
) -> bool {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let Some((_, b, g, r)) = parse_ass_hex_abgr(raw) else {
        return false;
    };
    let (nr, ng, nb) = conv.rgb_to_rgb(r, g, b);
    let buf = [
        b'&',
        b'H',
        HEX[(nb >> 4) as usize],
        HEX[(nb & 0x0F) as usize],
        HEX[(ng >> 4) as usize],
        HEX[(ng & 0x0F) as usize],
        HEX[(nr >> 4) as usize],
        HEX[(nr & 0x0F) as usize],
        b'&',
    ];
    if raw.as_bytes() != buf {
        *changed = true;
    }
    if let Ok(s) = std::str::from_utf8(&buf) {
        out.push_str(s);
    }
    true
}

#[inline]
fn strip_outer_parens(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    trimmed.strip_prefix('(')?.strip_suffix(')')
}

#[inline]
fn parse_paren_f64_2(raw: &str) -> Option<(f64, f64)> {
    let inner = strip_outer_parens(raw)?;
    let (a, b) = inner.split_once(',')?;
    if b.contains(',') {
        return None;
    }
    let x = a.trim().parse::<f64>().ok()?;
    let y = b.trim().parse::<f64>().ok()?;
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

#[inline]
fn parse_csv_f64_4(inner: &str) -> Option<[f64; 4]> {
    let mut vals = [0.0f64; 4];
    let mut count = 0usize;
    for part in inner.split(',') {
        if count >= 4 {
            return None;
        }
        let v = part.trim().parse::<f64>().ok()?;
        if !v.is_finite() {
            return None;
        }
        vals[count] = v;
        count += 1;
    }
    (count == 4).then_some(vals)
}

#[inline]
fn parse_paren_f64_up_to_6(raw: &str) -> Option<([f64; 6], usize)> {
    let inner = strip_outer_parens(raw)?;
    let mut vals = [0.0f64; 6];
    let mut count = 0usize;
    for part in inner.split(',') {
        if count >= 6 {
            return None;
        }
        let v = part.trim().parse::<f64>().ok()?;
        if !v.is_finite() {
            return None;
        }
        vals[count] = v;
        count += 1;
    }
    Some((vals, count))
}

fn parse_ass_hex_abgr(raw: &str) -> Option<(u8, u8, u8, u8)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let body = trimmed
        .strip_prefix('&')
        .unwrap_or(trimmed)
        .strip_prefix(['H', 'h'])
        .unwrap_or_else(|| trimmed.strip_prefix('&').unwrap_or(trimmed))
        .trim_end_matches('&');
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let val = u32::from_str_radix(body, 16).ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let a = ((val >> 24) & 0xFF) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let b = ((val >> 16) & 0xFF) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let g = ((val >> 8) & 0xFF) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let r = (val & 0xFF) as u8;
    Some((a, b, g, r))
}

fn convert_ass_style_colour(raw: &str, conv: YcbcrConverter) -> Option<String> {
    let (a, b, g, r) = parse_ass_hex_abgr(raw)?;
    let (nr, ng, nb) = conv.rgb_to_rgb(r, g, b);
    Some(format!("&H{a:02X}{nb:02X}{ng:02X}{nr:02X}"))
}

/// Scales `(x, y)` coordinate pairs inside an ASS vector drawing string (`m`, `n`, `l`, `b`, `s`, `p`, `c`),
/// appending the result to `out`.
fn scale_drawing_commands(
    out: &mut String,
    cmds: &str,
    rx: f64,
    ry: f64,
    shift_x: f64,
    shift_y: f64,
    scale_level: u32,
) {
    let scale_mul = f64::from(1u32 << scale_level.saturating_sub(1).min(16));
    let sx = shift_x * scale_mul;
    let sy = shift_y * scale_mul;

    let mut is_x = true;
    for (idx, tok) in cmds.split_whitespace().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let b0 = tok.as_bytes()[0];
        if tok.len() == 1 && b0.is_ascii_alphabetic() {
            // Drawing command letter resets x/y parity.
            is_x = true;
            out.push_str(tok);
        } else if let Ok(v) = tok.parse::<f64>() {
            let scaled = if is_x { v * rx + sx } else { v * ry + sy };
            write_num(out, scaled);
            is_x = !is_x;
        } else {
            is_x = true;
            out.push_str(tok);
        }
    }
}

fn round_2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[inline]
fn write_i64_ascii(out: &mut String, val: i64) {
    if val == 0 {
        out.push('0');
        return;
    }
    let mut abs = val.unsigned_abs();
    let mut buf = [0u8; 22];
    let mut pos = buf.len();
    while abs > 0 {
        pos -= 1;
        #[allow(clippy::cast_possible_truncation)]
        {
            buf[pos] = b'0' + (abs % 10) as u8;
        }
        abs /= 10;
    }
    if val < 0 {
        pos -= 1;
        buf[pos] = b'-';
    }
    if let Ok(s) = std::str::from_utf8(&buf[pos..]) {
        out.push_str(s);
    }
}

/// Appends `v` rounded to 2 decimals with trailing zeroes trimmed (integers print without a point).
#[inline]
fn write_num(out: &mut String, v: f64) {
    let scaled_f = (v * 100.0).round();
    if !scaled_f.is_finite() || scaled_f.abs() >= 1e14 {
        let r = round_2(v);
        let _ = write!(out, "{r}");
        return;
    }
    #[allow(clippy::cast_possible_truncation)]
    let scaled = scaled_f as i64;
    if scaled == 0 {
        out.push('0');
        return;
    }
    if scaled < 0 {
        out.push('-');
    }
    let abs = scaled.unsigned_abs();
    let whole = abs / 100;
    let frac = abs % 100;
    let mut buf = [0u8; 20];
    let mut pos = buf.len();
    if whole == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        let mut w = whole;
        while w > 0 {
            pos -= 1;
            #[allow(clippy::cast_possible_truncation)]
            {
                buf[pos] = b'0' + (w % 10) as u8;
            }
            w /= 10;
        }
    }
    if let Ok(s) = std::str::from_utf8(&buf[pos..]) {
        out.push_str(s);
    }
    if frac != 0 {
        out.push('.');
        #[allow(clippy::cast_possible_truncation)]
        let tens = (frac / 10) as u8;
        #[allow(clippy::cast_possible_truncation)]
        let ones = (frac % 10) as u8;
        out.push(char::from(b'0' + tens));
        if ones != 0 {
            out.push(char::from(b'0' + ones));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{parse_ass, UndoHistory};

    #[test]
    fn resamples_720p_to_1080p_styles_tags_and_drawings_and_undoes() {
        let raw = "\
[Script Info]
ScriptType: v4.00+
PlayResX: 1280
PlayResY: 720
LayoutResX: 1280
LayoutResY: 720

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,2,0,1,2,1,2,20,20,30,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:04.00,Default,,0,0,0,,{\\pos(640,360)\\fs40\\bord2\\shad1\\blur2\\t(0,500,\\fs60)\\clip(0,0,100,50)}Hello
Dialogue: 0,0:00:05.00,0:00:08.00,Default,,0,0,0,,{\\p1\\pbo10}m 0 0 l 100 0 100 50{\\p0}
";
        let mut script = parse_ass(raw).value;
        let mut opts = ResampleOptions::new(1920, 1080);
        opts.ycbcr_matrix = Some("TV.709".to_owned());

        let cmd = resample_resolution(&script, &opts).expect("resample");
        // Generating the command must not mutate [Script Info] prematurely.
        assert_eq!(script.script_info().unwrap().get("PlayResX"), Some("1280"));
        assert_eq!(script.script_info().unwrap().get("YCbCr Matrix"), None);

        let mut history = UndoHistory::default();
        history.execute(&mut script, cmd).expect("exec");

        assert_eq!(script.script_info().unwrap().get("PlayResX"), Some("1920"));
        assert_eq!(
            script.script_info().unwrap().get("LayoutResY"),
            Some("1080")
        );
        assert_eq!(
            script.script_info().unwrap().get("YCbCr Matrix"),
            Some("TV.709")
        );

        let st = script.styles().next().unwrap();
        assert!((st.fontsize() - 60.0).abs() < 1e-6);
        assert!((st.outline() - 3.0).abs() < 1e-6);
        assert!((st.shadow() - 1.5).abs() < 1e-6);
        assert_eq!(st.margin_l(), 30);
        assert_eq!(st.margin_v(), 45);

        let evs: Vec<_> = script.events().collect();
        assert_eq!(
            evs[0].text(),
            r"{\pos(960,540)\fs60\bord3\shad1.5\blur3\t(0,500,\fs90)\clip(0,0,150,75)}Hello"
        );
        assert_eq!(evs[1].text(), r"{\p1\pbo15}m 0 0 l 150 0 150 75{\p0}");

        // Undo reverts [Script Info], styles, and events in one step.
        history.undo(&mut script).expect("undo");
        assert_eq!(script.script_info().unwrap().get("PlayResX"), Some("1280"));
        assert_eq!(script.script_info().unwrap().get("PlayResY"), Some("720"));
        assert_eq!(script.script_info().unwrap().get("LayoutResY"), Some("720"));
        assert_eq!(script.script_info().unwrap().get("YCbCr Matrix"), None);
        assert!((script.styles().next().unwrap().fontsize() - 40.0).abs() < 1e-6);
        assert!(script
            .events()
            .next()
            .unwrap()
            .text()
            .contains(r"\pos(640,360)"));

        // Redo restores all header, style, and event mutations.
        history.redo(&mut script).expect("redo");
        assert_eq!(script.script_info().unwrap().get("PlayResX"), Some("1920"));
        assert_eq!(
            script.script_info().unwrap().get("YCbCr Matrix"),
            Some("TV.709")
        );
    }

    #[test]
    fn anamorphic_stretch_scales_scale_x_and_fscx() {
        // 4:3 (640x480) stretched to 16:9 (1920x1080): rx=3.0, ry=2.25, ar=4/3.
        let raw = "\
[Script Info]
ScriptType: v4.00+
PlayResX: 640
PlayResY: 480

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,10,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\fscx100\\pos(320,240)}Wide
";
        let mut script = parse_ass(raw).value;
        let mut opts = ResampleOptions::new(1920, 1080);
        opts.ar_mode = AspectRatioMode::Stretch;
        let cmd = resample_resolution(&script, &opts).expect("stretch");
        cmd.apply(&mut script).expect("apply");

        let st = script.styles().next().unwrap();
        assert!((st.scale_x() - 133.33).abs() < 0.02);
        let ev = script.events().next().unwrap();
        assert_eq!(ev.text(), r"{\fscx133.33\pos(960,540)}Wide");
    }

    #[test]
    fn ycbcr_matrix_change_converts_style_and_override_colours_including_transform_and_undoes() {
        let raw = "\
[Script Info]
ScriptType: v4.00+
PlayResX: 1280
PlayResY: 720
YCbCr Matrix: TV.601

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,40,&H0000FF00,&H400000FF,&H00FFFFFF,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,10,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\c&H00FF00&\\1c&H0000FF&\\2c&HFF0000&\\3c&HFFFFFF&\\4c&H000000&\\t(0,500,\\3c&H00FF00&)\\c}Green
";
        let mut script = parse_ass(raw).value;
        let mut opts = ResampleOptions::new(1280, 720);
        opts.ycbcr_matrix = Some("TV.709".to_owned());

        let mut history = UndoHistory::default();
        let cmd = resample_resolution(&script, &opts).expect("resample matrix");
        history.execute(&mut script, cmd).expect("exec");

        assert_eq!(
            script.script_info().unwrap().get("YCbCr Matrix"),
            Some("TV.709")
        );

        // Pure green (0,255,0) TV.601 -> TV.709 shifts channels; alpha byte 0x40 on red preserved.
        let expected_green_601_to_709 = YcbcrConverter::new(
            parse_ycbcr_colorspace("TV.601").unwrap(),
            parse_ycbcr_colorspace("TV.709").unwrap(),
        )
        .rgb_to_rgb(0, 255, 0);
        assert_ne!(expected_green_601_to_709, (0, 255, 0));

        let st = script.styles().next().unwrap();
        let expected_st_primary = format!(
            "&H00{:02X}{:02X}{:02X}",
            expected_green_601_to_709.2, expected_green_601_to_709.1, expected_green_601_to_709.0
        );
        assert_eq!(st.primary_colour(), expected_st_primary);
        assert!(st.secondary_colour().starts_with("&H40"));
        assert_ne!(st.secondary_colour(), "&H400000FF");
        // White and black stay neutral.
        assert_eq!(st.outline_colour(), "&H00FFFFFF");
        assert_eq!(st.back_colour(), "&H80000000");

        let ev_text = script.events().next().unwrap().text();
        let expected_override_green = format!(
            "&H{:02X}{:02X}{:02X}&",
            expected_green_601_to_709.2, expected_green_601_to_709.1, expected_green_601_to_709.0
        );
        assert!(ev_text.contains(&format!(r"\c{expected_override_green}")));
        assert!(ev_text.contains(&format!(r"\t(0,500,\3c{expected_override_green})")));
        assert!(ev_text.ends_with(r"\c}Green"));
        assert!(!ev_text.contains(r"\1c&H0000FF&"));
        assert!(!ev_text.contains(r"\2c&HFF0000&"));
        // White/black overrides untouched.
        assert!(ev_text.contains(r"\3c&HFFFFFF&"));
        assert!(ev_text.contains(r"\4c&H000000&"));

        // Round-trip back TV.709 -> TV.601 and verify undo restores exact originals.
        history.undo(&mut script).expect("undo");
        assert_eq!(
            script.script_info().unwrap().get("YCbCr Matrix"),
            Some("TV.601")
        );
        let st_restored = script.styles().next().unwrap();
        assert_eq!(st_restored.primary_colour(), "&H0000FF00");
        assert_eq!(st_restored.secondary_colour(), "&H400000FF");
        assert!(script
            .events()
            .next()
            .unwrap()
            .text()
            .contains(r"\t(0,500,\3c&H00FF00&)"));

        // In-gamut colours (80, 160, 120) round-trip TV.601 -> TV.709 -> TV.601 within <= 2/255.
        let conv_601_709 = YcbcrConverter::new(
            parse_ycbcr_colorspace("TV.601").unwrap(),
            parse_ycbcr_colorspace("TV.709").unwrap(),
        );
        let conv_709_601 = YcbcrConverter::new(
            parse_ycbcr_colorspace("TV.709").unwrap(),
            parse_ycbcr_colorspace("TV.601").unwrap(),
        );
        let (r1, g1, b1) = conv_601_709.rgb_to_rgb(80, 160, 120);
        let (r2, g2, b2) = conv_709_601.rgb_to_rgb(r1, g1, b1);
        assert!(
            r2.abs_diff(80) <= 2 && g2.abs_diff(160) <= 2 && b2.abs_diff(120) <= 2,
            "in-gamut round-trip=({r2},{g2},{b2})"
        );
    }

    #[test]
    fn drawings_omit_border_shift_and_stretch_uncompensated_while_clips_and_nonzero_event_margins_shift(
    ) {
        // 4:3 (640x480) -> 16:9 (1280x720) AddBorders: r = 1.5, border_x = 160, border_y = 0.
        let raw = "\
[Script Info]
ScriptType: v4.00+
PlayResX: 640
PlayResY: 480

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,10,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,20,0,0,,{\\clip(m 0 0 l 100 50)}Clipped
Dialogue: 0,0:00:02.00,0:00:03.00,Default,,0,0,0,,{\\p1}m 0 0 l 100 50{\\p0}
";
        let mut script_ab = parse_ass(raw).value;
        let opts_ab = ResampleOptions::new(1280, 720);
        resample_resolution(&script_ab, &opts_ab)
            .expect("resample")
            .apply(&mut script_ab)
            .expect("apply");

        let evs: Vec<_> = script_ab.events().collect();
        // Vector \clip gets border_x=160 shift; non-zero MarginL=20 shifts (20*1.5 + 160 = 190); 0 MarginR stays 0.
        assert_eq!(evs[0].text(), r"{\clip(m 160 0 l 310 75)}Clipped");
        assert_eq!(evs[0].margin_l(), 190);
        assert_eq!(evs[0].margin_r(), 0);
        // \p1 drawing does NOT get the border_x=160 shift (matches Aegisub resolution_resampler.cpp).
        assert_eq!(evs[1].text(), r"{\p1}m 0 0 l 150 75{\p0}");

        // In Stretch mode (640x480 -> 1920x1080: rx=3.0, ry=2.25, ar=4/3), \p1 X uses rx/ar == ry == 2.25.
        let mut script_st = parse_ass(raw).value;
        let mut opts_st = ResampleOptions::new(1920, 1080);
        opts_st.ar_mode = AspectRatioMode::Stretch;
        resample_resolution(&script_st, &opts_st)
            .expect("stretch")
            .apply(&mut script_st)
            .expect("apply");
        let st_evs: Vec<_> = script_st.events().collect();
        assert_eq!(st_evs[1].text(), r"{\p1}m 0 0 l 225 112.5{\p0}");
    }

    #[test]
    fn invalid_play_res_is_an_error_not_a_silent_default() {
        let script = parse_ass(
            "[Script Info]
ScriptType: v4.00+
PlayResX: abc
PlayResY: 480
",
        )
        .value;
        let err = resample_resolution(&script, &ResampleOptions::new(1920, 1080)).unwrap_err();
        assert!(
            matches!(
                err,
                ToolsError::InvalidSourceResolution {
                    key: "PlayResX",
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("PlayResX"));
        // An explicit override still works.
        let mut opts = ResampleOptions::new(1920, 1080);
        opts.source_override = Some((640, 480));
        assert!(resample_resolution(&script, &opts).is_ok());
    }
}
