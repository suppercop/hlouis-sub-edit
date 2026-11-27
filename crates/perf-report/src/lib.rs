//! Reads Criterion `estimates.json` files and `hlouis-ui-demo --bench-ui` output and renders
//! a colour-coded `docs/perf-report.md` comparing Measured vs. Target Ceiling vs. Aegisub 3.5.1.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Errors returned while collecting or writing performance reports.
#[derive(Debug, Error)]
pub enum PerfReportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// One benchmark row in the generated Markdown report.
#[derive(Debug, Clone)]
pub struct BenchmarkRow {
    pub name: String,
    pub measured_us: f64,
    pub ceiling_us: Option<f64>,
    pub aegisub_us: Option<f64>,
}

/// Status badge classification for a benchmark row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    Pass,
    Warn,
    Fail,
}

impl RowStatus {
    /// Classifies `measured_us` against `ceiling_us` (0 heap allocations).
    #[must_use]
    pub fn classify(measured_us: f64, ceiling_us: Option<f64>) -> Self {
        let Some(ceil) = ceiling_us else {
            return Self::Pass;
        };
        if measured_us > ceil {
            Self::Fail
        } else if measured_us > ceil * 0.85 {
            Self::Warn
        } else {
            Self::Pass
        }
    }

    /// Colour-coded Markdown badge.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Pass => "🟢 PASS",
            Self::Warn => "🟡 NEAR",
            Self::Fail => "🔴 OVER",
        }
    }
}

#[derive(Deserialize)]
struct CriterionEstimate {
    median: CriterionPoint,
}

#[derive(Deserialize)]
struct CriterionPoint {
    point_estimate: f64,
}

/// Recursively scans `criterion_root` (e.g. `target/criterion`) for `*/new/estimates.json`
/// and returns median timings in microseconds keyed by benchmark name.
///
/// # Errors
/// Returns [`PerfReportError`] if reading or parsing fails.
pub fn collect_criterion_estimates(
    criterion_root: &Path,
) -> Result<BTreeMap<String, f64>, PerfReportError> {
    let mut out = BTreeMap::new();
    if !criterion_root.exists() {
        return Ok(out);
    }
    walk_criterion(criterion_root, criterion_root, &mut out)?;
    Ok(out)
}

fn walk_criterion(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, f64>,
) -> Result<(), PerfReportError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_criterion(root, &path, out)?;
        } else if path.file_name().and_then(|n| n.to_str()) == Some("estimates.json")
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("new")
        {
            let raw = fs::read_to_string(&path)?;
            let est: CriterionEstimate = serde_json::from_str(&raw)?;
            let bench_name = path
                .parent()
                .and_then(Path::parent)
                .and_then(|p| p.strip_prefix(root).ok())
                .map_or_else(
                    || "unknown".to_owned(),
                    |rel| rel.to_string_lossy().replace('\\', "/"),
                );
            // Criterion stores nanoseconds; convert to microseconds.
            out.insert(bench_name, est.median.point_estimate / 1_000.0);
        }
    }
    Ok(())
}

/// Parsed UI metrics from `hlouis-ui-demo --bench-ui` stdout.
#[derive(Debug, Clone, Default)]
pub struct BenchUiSummary {
    pub frame_p50_us: Option<f64>,
    pub frame_p95_us: Option<f64>,
    pub frame_p99_us: Option<f64>,
    pub frame_worst_us: Option<f64>,
    pub keystroke_p95_us: Option<f64>,
    pub seek_p95_us: Option<f64>,
}

/// Parses the text table emitted by `hlouis-ui-demo --bench-ui`.
#[must_use]
pub fn parse_bench_ui_output(text: &str) -> BenchUiSummary {
    let mut summary = BenchUiSummary::default();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 6 {
            match cols[0] {
                "frame_time" => {
                    summary.frame_p50_us = cols[2].parse().ok();
                    summary.frame_p95_us = cols[3].parse().ok();
                    summary.frame_p99_us = cols[4].parse().ok();
                    summary.frame_worst_us = cols[5].parse().ok();
                }
                "keystroke_to_present" => {
                    summary.keystroke_p95_us = cols[3].parse().ok();
                }
                "seek_latency" => {
                    summary.seek_p95_us = cols[3].parse().ok();
                }
                _ => {}
            }
        }
    }
    summary
}

/// Known target ceilings (µs) and Aegisub 3.5.1 reference baselines (µs).
#[must_use]
pub fn known_reference(name: &str) -> (Option<f64>, Option<f64>) {
    match name {
        "grid_model_incremental_overlap_update_1_row" => (Some(10.0), Some(1_200.0)),
        "hlouis_clean_whitespace_200k" => (Some(40_000.0), Some(250_000.0)),
        "template_doctor_lint_2000_lines" => (Some(20_000.0), None),
        "kara_templater_500_syls_x_10_templates" => (Some(200_000.0), Some(900_000.0)),
        "spectrum_24min/spectrum_build_24min_48khz_mono" => (Some(1_500_000.0), Some(6_000_000.0)),
        "spectrum_scroll_tile_256x128" => (Some(200.0), Some(2_500.0)),
        "commands_hotkey_lookup" => (Some(1.0), Some(15.0)),
        "ui_shell_full_frame_200k_lines" => (Some(4_000.0), Some(16_000.0)),
        _ => (None, None),
    }
}

fn fmt_us(v: f64) -> String {
    if v >= 1_000_000.0 {
        format!("{:.2} s", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.2} ms", v / 1_000.0)
    } else if v >= 1.0 {
        format!("{v:.1} µs")
    } else {
        format!("{:.1} ns", v * 1_000.0)
    }
}

/// Renders the complete `docs/perf-report.md` Markdown report.
#[must_use]
pub fn render_markdown_report(rows: &[BenchmarkRow], ui: Option<&BenchUiSummary>) -> String {
    let mut md = String::from(
        "# HLouis Sub Edit — Performance Report\n\n\
         | Benchmark | Measured | Target Ceiling | Aegisub 3.5.1 Ref | Status |\n\
         |---|---:|---:|---:|---|\n",
    );
    for r in rows {
        let status = RowStatus::classify(r.measured_us, r.ceiling_us);
        let ceil = r.ceiling_us.map_or_else(|| "—".to_owned(), fmt_us);
        let aeg = r.aegisub_us.map_or_else(|| "—".to_owned(), fmt_us);
        let _ = writeln!(
            md,
            "| `{}` | **{}** | {} | {} | {} |",
            r.name,
            fmt_us(r.measured_us),
            ceil,
            aeg,
            status.badge()
        );
    }

    if let Some(u) = ui {
        md.push_str("\n## Interactive UI Benchmark (`hlouis-ui-demo --bench-ui`, 200k lines)\n\n");
        md.push_str("| Metric | Measured | Target Ceiling | Status |\n|---|---:|---:|---|\n");
        if let Some(v) = u.frame_p99_us {
            let _ = writeln!(
                md,
                "| `frame_time` p99 | **{}** | 4.00 ms | {} |",
                fmt_us(v),
                RowStatus::classify(v, Some(4_000.0)).badge()
            );
        }
        if let Some(v) = u.keystroke_p95_us {
            let _ = writeln!(
                md,
                "| `keystroke_to_present` p95 | **{}** | 8.00 ms | {} |",
                fmt_us(v),
                RowStatus::classify(v, Some(8_000.0)).badge()
            );
        }
        if let Some(v) = u.seek_p95_us {
            let _ = writeln!(
                md,
                "| `seek_latency` p95 | **{}** | 30.00 ms | {} |",
                fmt_us(v),
                RowStatus::classify(v, Some(30_000.0)).badge()
            );
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_criterion_json_and_bench_ui_into_markdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let est_dir = dir.path().join("commands_hotkey_lookup/new");
        fs::create_dir_all(&est_dir).expect("mkdir");
        fs::write(
            est_dir.join("estimates.json"),
            r#"{"median":{"point_estimate":10.8}}"#,
        )
        .expect("write");

        let map = collect_criterion_estimates(dir.path()).expect("collect");
        assert!(map.contains_key("commands_hotkey_lookup"));
        let rows: Vec<BenchmarkRow> = map
            .into_iter()
            .map(|(name, measured_us)| {
                let (ceiling_us, aegisub_us) = known_reference(&name);
                BenchmarkRow {
                    name,
                    measured_us,
                    ceiling_us,
                    aegisub_us,
                }
            })
            .collect();

        let ui_out = "frame_time 460 1824 2496 2624 3900\nkeystroke_to_present 220 1824 1888 1888 3000\nseek_latency 120 1312 2496 2496 3796\n";
        let ui = parse_bench_ui_output(ui_out);
        let md = render_markdown_report(&rows, Some(&ui));
        assert!(md.contains("commands_hotkey_lookup"));
        assert!(md.contains("🟢 PASS"));
        assert!(md.contains("frame_time"));
    }
}
