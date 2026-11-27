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
use ass_core::{
    parse_text_blocks, AssStyle, EditCommand, OverrideItem, OverrideTag, ScriptFile, TagArgs,
    TextBlock,
};
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
    shift_y: f64,
}

/// Resamples `script` to the target resolution in `opts`.
///
/// Updates `[Script Info]` (`PlayResX`, `PlayResY`, `LayoutResX`/`LayoutResY` when present, and
/// `YCbCr Matrix` when requested) and returns one atomic [`EditCommand::Batch`] covering all
/// style and event mutations.
pub fn resample_resolution(
    script: &mut ScriptFile,
    opts: &ResampleOptions,
) -> Result<EditCommand, ToolsError> {
    if opts.dest_x == 0 || opts.dest_y == 0 {
        return Err(ToolsError::InvalidResolution(opts.dest_x, opts.dest_y));
    }

    let (src_x, src_y) = resolve_source_res(script, opts.source_override);
    let state = compute_state(src_x, src_y, opts);

    // Update [Script Info] header values.
    if let Some(info) = script.script_info_mut() {
        info.set("PlayResX", opts.dest_x.to_string());
        info.set("PlayResY", opts.dest_y.to_string());
        if info.get("LayoutResX").is_some() {
            info.set("LayoutResX", opts.dest_x.to_string());
        }
        if info.get("LayoutResY").is_some() {
            info.set("LayoutResY", opts.dest_y.to_string());
        }
        if let Some(ref matrix) = opts.ycbcr_matrix {
            info.set("YCbCr Matrix", matrix.clone());
        }
    }

    let mut cmds = Vec::new();

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
    if let Some(events_sec) = script.events_section() {
        for (idx, ev) in events_sec.events().enumerate() {
            let margin_left = scale_margin(ev.margin_l(), state.rx, 0.0);
            let margin_right = scale_margin(ev.margin_r(), state.rx, 0.0);
            let margin_vert = scale_margin(ev.margin_v(), state.ry, 0.0);

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

fn resolve_source_res(script: &ScriptFile, override_res: Option<(u32, u32)>) -> (u32, u32) {
    if let Some((x, y)) = override_res {
        if x > 0 && y > 0 {
            return (x, y);
        }
    }
    let sx = script
        .script_info()
        .and_then(|s| s.get("PlayResX"))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(640);
    let sy = script
        .script_info()
        .and_then(|s| s.get("PlayResY"))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(480);
    (sx, sy)
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
        shift_y: f64::from(opts.margin_top) * ry + border_y,
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
    out.set_margin_r(scale_margin(st.margin_r(), s.rx, s.shift_x));
    out.set_margin_v(scale_margin(st.margin_v(), s.ry, s.shift_y));
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
    let blocks = parse_text_blocks(text);
    let mut out = String::with_capacity(text.len() + 32);
    let mut changed = false;

    for block in &blocks {
        match block {
            TextBlock::Override { items, closed } => {
                out.push('{');
                for item in items {
                    write_resampled_item(&mut out, item, state, &mut changed);
                }
                if *closed {
                    out.push('}');
                }
            }
            TextBlock::Plain(s) => out.push_str(s),
            TextBlock::Escape(esc) => out.push_str(esc.as_str()),
            TextBlock::Drawing { level, commands } => {
                let scaled = scale_drawing_commands(commands, state.rx, state.ry, 0.0, 0.0, *level);
                if scaled != *commands {
                    changed = true;
                }
                out.push_str(&scaled);
            }
        }
    }

    if changed {
        Some(out)
    } else {
        None
    }
}

fn write_resampled_item(
    out: &mut String,
    item: &OverrideItem,
    state: ResampleState,
    changed: &mut bool,
) {
    match item {
        OverrideItem::Comment(c) => out.push_str(c),
        OverrideItem::Tag(tag) => write_resampled_tag(out, tag, state, changed),
    }
}

fn write_resampled_tag(out: &mut String, tag: &OverrideTag, s: ResampleState, changed: &mut bool) {
    out.push('\\');
    out.push_str(&tag.name);

    match &tag.args {
        TagArgs::Point { x, y } => {
            *changed = true;
            let nx = x * s.rx + s.shift_x;
            let ny = y * s.ry + s.shift_y;
            let _ = write!(out, "({},{})", fmt_num(nx), fmt_num(ny));
        }
        TagArgs::Move {
            x1,
            y1,
            x2,
            y2,
            t1,
            t2,
        } => {
            *changed = true;
            let nx1 = x1 * s.rx + s.shift_x;
            let ny1 = y1 * s.ry + s.shift_y;
            let nx2 = x2 * s.rx + s.shift_x;
            let ny2 = y2 * s.ry + s.shift_y;
            if let (Some(a), Some(b)) = (t1, t2) {
                let _ = write!(
                    out,
                    "({},{},{},{},{a},{b})",
                    fmt_num(nx1),
                    fmt_num(ny1),
                    fmt_num(nx2),
                    fmt_num(ny2)
                );
            } else {
                let _ = write!(
                    out,
                    "({},{},{},{})",
                    fmt_num(nx1),
                    fmt_num(ny1),
                    fmt_num(nx2),
                    fmt_num(ny2)
                );
            }
        }
        TagArgs::ClipRect { x1, y1, x2, y2, .. } => {
            *changed = true;
            let _ = write!(
                out,
                "({},{},{},{})",
                fmt_num(x1 * s.rx + s.shift_x),
                fmt_num(y1 * s.ry + s.shift_y),
                fmt_num(x2 * s.rx + s.shift_x),
                fmt_num(y2 * s.ry + s.shift_y)
            );
        }
        TagArgs::ClipVector {
            scale, commands, ..
        } => {
            *changed = true;
            let scaled = scale_drawing_commands(commands, s.rx, s.ry, s.shift_x, s.shift_y, *scale);
            if *scale == 1 {
                let _ = write!(out, "({scaled})");
            } else {
                let _ = write!(out, "({scale},{scaled})");
            }
        }
        TagArgs::Transform {
            t1,
            t2,
            accel,
            items,
        } => {
            let mut inner = String::new();
            let mut inner_changed = false;
            for item in items {
                write_resampled_item(&mut inner, item, s, &mut inner_changed);
            }
            if inner_changed {
                *changed = true;
                out.push('(');
                if let (Some(a), Some(b)) = (t1, t2) {
                    let _ = write!(out, "{a},{b},");
                }
                if let Some(acc) = accel {
                    let _ = write!(out, "{},", fmt_num(*acc));
                }
                out.push_str(&inner);
                out.push(')');
            } else {
                out.push_str(&tag.raw_args);
            }
        }
        TagArgs::Raw(raw) => {
            if let Some(scaled) = scale_scalar_tag(&tag.name, raw, s) {
                *changed = true;
                out.push_str(&scaled);
            } else {
                out.push_str(raw);
            }
        }
        _ => out.push_str(&tag.raw_args),
    }
}

fn scale_scalar_tag(name: &str, raw: &str, s: ResampleState) -> Option<String> {
    let factor = match name {
        "fs" | "bord" | "ybord" | "shad" | "yshad" | "be" | "blur" | "pbo" => s.ry,
        "fsp" | "xbord" | "xshad" => s.rx,
        "fscx" if (s.ar - 1.0).abs() > 1e-6 => s.ar,
        _ => return None,
    };
    let val = raw.trim().parse::<f64>().ok()?;
    if !val.is_finite() {
        return None;
    }
    Some(fmt_num(val * factor))
}

/// Scales `(x, y)` coordinate pairs inside an ASS vector drawing string (`m`, `n`, `l`, `b`, `s`, `p`, `c`).
fn scale_drawing_commands(
    cmds: &str,
    rx: f64,
    ry: f64,
    shift_x: f64,
    shift_y: f64,
    scale_level: u32,
) -> String {
    let scale_mul = f64::from(1u32 << scale_level.saturating_sub(1).min(16));
    let sx = shift_x * scale_mul;
    let sy = shift_y * scale_mul;

    let mut out = String::with_capacity(cmds.len() + 16);
    let mut is_x = true;
    for (idx, tok) in cmds.split_whitespace().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        if let Ok(v) = tok.parse::<f64>() {
            let scaled = if is_x { v * rx + sx } else { v * ry + sy };
            out.push_str(&fmt_num(scaled));
            is_x = !is_x;
        } else {
            // Drawing command letter resets x/y parity.
            is_x = true;
            out.push_str(tok);
        }
    }
    out
}

fn round_2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn fmt_num(v: f64) -> String {
    let r = round_2(v);
    if (r - r.round()).abs() < 1e-6 && r.abs() < 1e12 {
        #[allow(clippy::cast_possible_truncation)]
        let int_v = r.round() as i64;
        int_v.to_string()
    } else {
        // Trim trailing zeroes after decimal point.
        let s = format!("{r:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
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

        let cmd = resample_resolution(&mut script, &opts).expect("resample");
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

        // Undo reverts styles and events in one step.
        history.undo(&mut script).expect("undo");
        assert!((script.styles().next().unwrap().fontsize() - 40.0).abs() < 1e-6);
        assert!(script
            .events()
            .next()
            .unwrap()
            .text()
            .contains(r"\pos(640,360)"));
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
        let cmd = resample_resolution(&mut script, &opts).expect("stretch");
        cmd.apply(&mut script).expect("apply");

        let st = script.styles().next().unwrap();
        assert!((st.scale_x() - 133.33).abs() < 0.02);
        let ev = script.events().next().unwrap();
        assert_eq!(ev.text(), r"{\fscx133.33\pos(960,540)}Wide");
    }
}
