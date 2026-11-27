//! CLI generating `docs/perf-report.md` from Criterion JSON + `--bench-ui` stdout.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use perf_report::{
    BenchmarkRow, collect_criterion_estimates, known_reference, parse_bench_ui_output,
    render_markdown_report,
};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let criterion_dir = args
        .iter()
        .position(|a| a == "--criterion")
        .and_then(|i| args.get(i + 1))
        .map_or_else(|| PathBuf::from("target/criterion"), PathBuf::from);
    let bench_ui_file = args
        .iter()
        .position(|a| a == "--bench-ui-log")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let output_path = args
        .iter()
        .position(|a| a == "--output")
        .and_then(|i| args.get(i + 1))
        .map_or_else(|| PathBuf::from("docs/perf-report.md"), PathBuf::from);

    let estimates = match collect_criterion_estimates(&criterion_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("perf-report: failed to read criterion estimates: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rows: Vec<BenchmarkRow> = estimates
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
    let ui_summary = bench_ui_file
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|raw| parse_bench_ui_output(&raw));

    let md = render_markdown_report(&rows, ui_summary.as_ref());
    if let Some(parent) = output_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&output_path, md) {
        eprintln!(
            "perf-report: failed to write {}: {e}",
            output_path.display()
        );
        return ExitCode::FAILURE;
    }
    println!("Wrote performance report to {}", output_path.display());
    ExitCode::SUCCESS
}
