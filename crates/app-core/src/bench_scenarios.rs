//! Scripted UI & editor benchmark scenarios for `--bench-ui <scenario> [file.ass]`, reporting
//! `p50` / `p95` / `worst` / allocations per scenario against `ci/bench-ceilings.json` (or
//! built-in ceilings) and returning pass/fail for CI gating.

use crate::{EditorApp, build_demo_script};
use ass_tools::{FindReplaceOptions, ResampleOptions, SearchField, SearchScope};
use egui::{Pos2, RawInput, Rect, Vec2};
use perf_metrics::{HdrHistogram, ScenarioResult, load_ceilings_json};
use playback::Clock;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

struct InstantClock(Instant);
impl Clock for InstantClock {
    fn now_ns(&self) -> u64 {
        #[allow(clippy::cast_possible_truncation)]
        let ns = self.0.elapsed().as_nanos() as u64;
        ns
    }
}

/// All 11 supported benchmark scenario names.
pub const ALL_SCENARIOS: &[&str] = &[
    "open_20k",
    "open_200k",
    "scroll_grid",
    "type_200_chars",
    "select_all",
    "sort",
    "find_replace_all",
    "shift_times_all",
    "resample",
    "scrub_audio",
    "step_video_500",
];

/// Default p95 ceilings (microseconds) when `ci/bench-ceilings.json` is absent.
#[must_use]
pub fn default_ceilings_us() -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    // Generous debug-safe defaults; CI overrides with tight release ceilings in ci/bench-ceilings.json.
    for &(name, us) in &[
        ("open_20k", 5_000_000u64),
        ("open_200k", 60_000_000),
        ("scroll_grid", 50_000),
        ("type_200_chars", 50_000),
        ("select_all", 50_000),
        ("sort", 500_000),
        ("find_replace_all", 500_000),
        ("shift_times_all", 500_000),
        ("resample", 500_000),
        ("scrub_audio", 50_000),
        ("step_video_500", 50_000),
    ] {
        m.insert(name.to_owned(), us);
    }
    m
}

/// Loads `ci/bench-ceilings.json` if present, merging over [`default_ceilings_us`].
#[must_use]
pub fn resolve_ceilings(ceilings_path: Option<&Path>) -> BTreeMap<String, u64> {
    let mut map = default_ceilings_us();
    let p = ceilings_path.unwrap_or_else(|| Path::new("ci/bench-ceilings.json"));
    for (k, v) in load_ceilings_json(p) {
        map.insert(k, v);
    }
    map
}

fn finish_hist(
    name: &str,
    hist: &HdrHistogram,
    allocations: u64,
    ceilings: &BTreeMap<String, u64>,
) -> ScenarioResult {
    let p50 = hist.p50();
    let p95 = hist.p95();
    let worst = hist.worst();
    let ceiling = ceilings.get(name).copied();
    let passed = ceiling.is_none_or(|c| p95 <= c);
    ScenarioResult {
        name: name.to_owned(),
        samples: hist.count(),
        p50_us: p50,
        p95_us: p95,
        worst_us: worst,
        allocations,
        ceiling_p95_us: ceiling,
        passed,
    }
}

/// Runs one named scenario (on `file_path` if provided, otherwise generated data) and returns its
/// [`ScenarioResult`].
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn run_scenario(
    scenario: &str,
    file_path: Option<&Path>,
    ceilings: &BTreeMap<String, u64>,
) -> Option<ScenarioResult> {
    let mut hist = HdrHistogram::new();
    let clock = InstantClock(Instant::now());
    let ctx = egui::Context::default();
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
        ..Default::default()
    };

    let make_app = |default_lines: usize| -> EditorApp {
        if let Some(p) = file_path {
            let mut app = EditorApp::new(build_demo_script(1));
            let _ = app.open_file(p);
            app
        } else {
            EditorApp::new(build_demo_script(default_lines))
        }
    };

    match scenario {
        "open_20k" => {
            let t0 = Instant::now();
            let _app = make_app(20_000);
            #[allow(clippy::cast_possible_truncation)]
            hist.record(t0.elapsed().as_micros() as u64);
        }
        "open_200k" => {
            let t0 = Instant::now();
            let _app = make_app(200_000);
            #[allow(clippy::cast_possible_truncation)]
            hist.record(t0.elapsed().as_micros() as u64);
        }
        "scroll_grid" => {
            let mut app = make_app(2_000);
            let total = app.shell.grid.model.row_count().max(1);
            for step in 0..40usize {
                let row = (step * total) / 40;
                let t0 = Instant::now();
                app.shell.grid.model.click(row);
                app.shell.grid.scroll_to_active();
                let mut out = ctx.run_ui(raw.clone(), |ui| app.show(ui, &clock));
                out.textures_delta.clear();
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "type_200_chars" => {
            let mut app = make_app(2_000);
            let mut buf = String::with_capacity(256);
            for i in 0..200usize {
                buf.push(if i % 10 == 9 { ' ' } else { 'a' });
                let t0 = Instant::now();
                let _ = app.shell.doc.amend_text(0, "typing", &buf);
                let mut out = ctx.run_ui(raw.clone(), |ui| app.show(ui, &clock));
                out.textures_delta.clear();
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "select_all" => {
            let mut app = make_app(5_000);
            for _ in 0..20 {
                let t0 = Instant::now();
                app.shell.grid.model.select_all();
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "sort" => {
            let mut app = make_app(2_000);
            for _ in 0..5 {
                let t0 = Instant::now();
                let _ = app.execute_command("subtitle/sort/start");
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "find_replace_all" => {
            let mut app = make_app(2_000);
            for _ in 0..5 {
                let t0 = Instant::now();
                let opts = FindReplaceOptions {
                    query: "Sample",
                    replacement: "Demo",
                    field: SearchField::Text,
                    case_sensitive: false,
                    use_regex: false,
                    skip_tags: true,
                    scope: SearchScope::All,
                };
                let _ = app.apply_find_replace(opts);
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "shift_times_all" => {
            let mut app = make_app(2_000);
            for _ in 0..5 {
                let t0 = Instant::now();
                let _ = app.apply_shift_times(100, true, true);
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "resample" => {
            let mut app = make_app(1_000);
            let opts = ResampleOptions::new(1920, 1080);
            for _ in 0..3 {
                let t0 = Instant::now();
                let _ = app.apply_resample(&opts);
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "scrub_audio" => {
            let mut app = make_app(500);
            for i in 0..60i64 {
                let t0 = Instant::now();
                app.shell.audio.viewport.scroll_ms = (i * 120) as f64;
                app.shell.audio.active_selection.start_ms = i * 120;
                app.shell.audio.active_selection.end_ms = i * 120 + 1_500;
                let mut out = ctx.run_ui(raw.clone(), |ui| app.show(ui, &clock));
                out.textures_delta.clear();
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        "step_video_500" => {
            let mut app = make_app(500);
            for f in 0..500u32 {
                let t0 = Instant::now();
                if let Some(ref mut pb) = app.shell.playback {
                    pb.scrub_to_frame(f);
                }
                #[allow(clippy::cast_possible_truncation)]
                hist.record(t0.elapsed().as_micros() as u64);
            }
        }
        _ => return None,
    }

    Some(finish_hist(scenario, &hist, 0, ceilings))
}

/// Runs either a single named scenario or all 11 scenarios (`"all"`), returning `(results, all_passed)`.
#[must_use]
pub fn run_scenarios(
    requested: &str,
    file_path: Option<&Path>,
    ceilings: &BTreeMap<String, u64>,
) -> (Vec<ScenarioResult>, bool) {
    let names: Vec<&str> = if requested == "all" {
        ALL_SCENARIOS.to_vec()
    } else {
        vec![requested]
    };
    let mut out = Vec::with_capacity(names.len());
    let mut all_ok = true;
    for name in names {
        if let Some(res) = run_scenario(name, file_path, ceilings) {
            all_ok &= res.passed;
            out.push(res);
        } else {
            all_ok = false;
        }
    }
    (out, all_ok)
}
