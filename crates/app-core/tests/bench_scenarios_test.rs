//! Tests for scripted benchmark scenarios, `ci/bench-ceilings.json` loading, and fail-on-miss gating.

#![forbid(unsafe_code)]

use app_core::bench_scenarios::{resolve_ceilings, run_scenario, run_scenarios};
use perf_metrics::format_scenario_table;
use std::fs;
use tempfile::tempdir;

#[test]
fn bench_scenarios_run_format_table_and_gate_on_ceilings() {
    let ceilings = resolve_ceilings(None);
    let mut results = Vec::new();
    for name in [
        "select_all",
        "sort",
        "find_replace_all",
        "shift_times_all",
        "resample",
        "step_video_500",
    ] {
        let r = run_scenario(name, None, &ceilings).expect("known scenario");
        assert!(r.samples > 0);
        assert!(r.passed, "scenario {name} exceeded default ceiling: {r:?}");
        results.push(r);
    }

    let table = format_scenario_table(&results);
    assert!(table.contains("select_all"));
    assert!(table.contains("step_video_500"));
    assert!(table.contains("PASS"));

    // Verify ci/bench-ceilings.json parsing and non-zero failure when a ceiling is missed.
    let dir = tempdir().expect("tempdir");
    let ceil_file = dir.path().join("bench-ceilings.json");
    fs::write(&ceil_file, r#"{ "shift_times_all": 0 }"#).expect("write ceilings");
    let strict = resolve_ceilings(Some(&ceil_file));
    let (res, all_ok) = run_scenarios("shift_times_all", None, &strict);
    assert_eq!(res.len(), 1);
    assert!(!all_ok || res[0].p95_us == 0);
}
