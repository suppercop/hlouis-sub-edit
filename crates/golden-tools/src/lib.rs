//! Golden-output regression harness comparing `ass-tools` (Shift Times, Timing Post-Processor,
//! Resample Resolution) against expected `.ass` files verified against Aegisub 3.5.1.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{ScriptFile, parse_ass, serialize_ass};
use ass_tools::{
    AdjacentLinkConfig, AspectRatioMode, KeyframeSnapConfig, ResampleOptions, ShiftAffect,
    ShiftAmount, ShiftDirection, ShiftOptions, ShiftTarget, TppOptions, resample_resolution,
    shift_times, timing_post_process,
};
use std::collections::BTreeSet;

/// All 10 golden fixture cases covered by the regression harness.
pub const GOLDEN_CASES: [&str; 10] = [
    "01_shift_forward_2s",
    "02_shift_backward_clamp_zero",
    "03_shift_start_only_selected",
    "04_shift_end_only_onward",
    "05_tpp_lead_in_out",
    "06_tpp_make_adjacent_midpoint",
    "07_tpp_keyframe_snap",
    "08_resample_720p_to_1080p",
    "09_resample_1080p_to_720p",
    "10_resample_stretch_pos_clip",
];

/// Runs the named golden case on `input_ass` and returns the serialized output `.ass`.
///
/// # Errors
/// Returns an error string if the case name is unknown or applying the command fails.
#[allow(clippy::too_many_lines)]
pub fn run_golden_case(case_name: &str, input_ass: &str) -> Result<String, String> {
    let mut script: ScriptFile = parse_ass(input_ass).value;
    match case_name {
        "01_shift_forward_2s" => {
            let cmd = shift_times(
                &script,
                ShiftOptions {
                    amount: ShiftAmount::TimeCs(200),
                    direction: ShiftDirection::Forward,
                    target: ShiftTarget::Both,
                    affect: ShiftAffect::All,
                },
            )
            .map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "02_shift_backward_clamp_zero" => {
            let cmd = shift_times(
                &script,
                ShiftOptions {
                    amount: ShiftAmount::TimeCs(500),
                    direction: ShiftDirection::Backward,
                    target: ShiftTarget::Both,
                    affect: ShiftAffect::All,
                },
            )
            .map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "03_shift_start_only_selected" => {
            let mut sel = BTreeSet::new();
            sel.insert(1);
            let cmd = shift_times(
                &script,
                ShiftOptions {
                    amount: ShiftAmount::TimeCs(50),
                    direction: ShiftDirection::Forward,
                    target: ShiftTarget::Start,
                    affect: ShiftAffect::Selected(&sel),
                },
            )
            .map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "04_shift_end_only_onward" => {
            let cmd = shift_times(
                &script,
                ShiftOptions {
                    amount: ShiftAmount::TimeCs(100),
                    direction: ShiftDirection::Forward,
                    target: ShiftTarget::End,
                    affect: ShiftAffect::SelectionOnward(1),
                },
            )
            .map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "05_tpp_lead_in_out" => {
            let cmd = timing_post_process(
                &script,
                TppOptions {
                    lead_in_ms: Some(100),
                    lead_out_ms: Some(200),
                    skip_comments: true,
                    ..TppOptions::default()
                },
            );
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "06_tpp_make_adjacent_midpoint" => {
            let cmd = timing_post_process(
                &script,
                TppOptions {
                    adjacent: Some(AdjacentLinkConfig {
                        threshold_ms: 500,
                        bias: 0.5,
                        prevent_overlap: true,
                    }),
                    skip_comments: true,
                    ..TppOptions::default()
                },
            );
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "07_tpp_keyframe_snap" => {
            let kfs = [200u32, 500, 900];
            let cmd = timing_post_process(
                &script,
                TppOptions {
                    keyframes: Some(KeyframeSnapConfig {
                        keyframes_cs: &kfs,
                        start_before_ms: 150,
                        start_after_ms: 150,
                        end_before_ms: 150,
                        end_after_ms: 150,
                    }),
                    skip_comments: true,
                    ..TppOptions::default()
                },
            );
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "08_resample_720p_to_1080p" => {
            let cmd = resample_resolution(&script, &ResampleOptions::new(1920, 1080))
                .map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "09_resample_1080p_to_720p" => {
            let cmd = resample_resolution(&script, &ResampleOptions::new(1280, 720))
                .map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        "10_resample_stretch_pos_clip" => {
            let mut opts = ResampleOptions::new(1920, 1080);
            opts.ar_mode = AspectRatioMode::Stretch;
            let cmd = resample_resolution(&script, &opts).map_err(|e| e.to_string())?;
            cmd.apply(&mut script).map_err(|e| e.to_string())?;
        }
        other => return Err(format!("unknown golden case: {other}")),
    }
    Ok(serialize_ass(&script))
}
