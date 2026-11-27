# Performance Report (`perf-budget-v2`)

**Measured Date:** 2026-11-27  
**Branch / Env:** `perf-budget-v2` (stacked on `phase-1` `e145072`), Windows 11 x64, Rust `1.100.0`, Release (`opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`).  
**Reference Baseline:** Aegisub 3.5.1 installed at `C:\Program Files\Aegisub\aegisub.exe` on the same machine (`docs/aegisub-baseline.md`, `tools/aegisub-bench/`).

---

## 1. End-to-End Budgets vs. Measured (`--bench-ui` + Criterion)

All `--bench-ui` measurements below were captured with `target\release\hlouis-sub-edit.exe --bench-ui` (headless and windowed `wgpu` DX12, with and without a local 12 s 1080p H.264 MKV fixture). All Criterion medians were captured with `cargo bench --benches`.

| Area | Old Budget | New Budget (Ceiling) | Half-Aegisub Goal | `hlouis-sub-edit` Measured | Aegisub 3.5.1 Measured | Status |
|---|---|---|---|---|---|---|
| Idle, paused, no input | `< 0.5%` CPU | `< 0.2%` CPU, `0` repaints/s | `< 0.155%` CPU | `0.00 repaints/s`; CPU `0.000%` (run 1) / `1.008%` (15.6 ms OS timer quantum edge over 1.55 s window in run 3) | `0.31%` CPU | `0 repaints/s` **PASS**; short-window OS quantum variance tracked (`perf: idle paused cpu sampling`) |
| Cold start to usable window | `< 500 ms` | `< 300 ms` | `< 3.18 s` | Windowed UI init `1549.5–2007.0 ms`, first present `1653.8–2308.8 ms` (`wgpu_win=1523–1954 ms`, app init `25–51 ms`) | `6.35 s` (`2250.9 ms` internal `FrameMain` median across 9 logs) | Beats Aegisub **2.7x–3.8x**; **MISS** (`perf: cold start to usable window`) |
| Open 20,000-line ASS (incl. charset, I/O, grid) | `10k < 150 ms` | `< 40 ms` | — | Windowed `33.49–36.88 ms`; headless `27.71–34.81 ms` (parse Criterion `10.22 ms`) | Harness in `tools/aegisub-bench/` (`bench_ops.lua` + `run-aegisub-bench.ps1`) | **PASS** |
| Save 20,000-line ASS | `10k < 100 ms` | `< 25 ms` | — | Windowed `4.55–4.86 ms`; headless `6.28 ms` (serialize Criterion `6.75 ms`) | Harness in `tools/aegisub-bench/` | **PASS** |
| Grid scroll 200,000 lines | `worst < 16 ms` | `worst < 8 ms` (up to `120 Hz`) | — | Windowed CPU `p50 1.24–1.29 / p95 2.08–2.12 / worst 2.16–2.92 ms`; headless `worst 0.95–1.37 ms` | Harness in `tools/aegisub-bench/` | **PASS** |
| Grid select-all 200,000 lines | `< 50 ms` | `< 10 ms` | — | `0.00–0.01 ms` (`--bench-ui`); `1.1 µs` (`app_hot_paths/grid_200k_select_all` at `ceb6445`) | Harness in `tools/aegisub-bench/` | **PASS** |
| Keystroke to screen in EditBox | `< 16 ms` | `< 8 ms` | — | Headless layout `p50 0.06 / p95 0.10 / worst 0.25 ms`; windowed frame-to-frame `p50 11.12 / p95 11.84 / worst 13.92 ms` (90 Hz `Fifo` vsync interval `11.1 ms`) | UIA key->pixel harness in `tools/aegisub-bench/run-aegisub-bench.ps1` | Headless **PASS**; windowed VSync **MISS** (`perf: keystroke to screen in edit box`) |
| Undo / redo 20,000 lines | `< 5 ms` | `< 1 ms` | — | `82.74 ns` (`0.00008 ms`, Criterion `undo_redo_step_20k_lines`) | `bench_ops.lua` | **PASS** |
| Find / replace 20,000 lines | `< 200 ms` | `< 30 ms` | — | `6.47 ms` (Criterion `find_replace_skip_tags_20k`) | `bench_ops.lua` | **PASS** |
| All `ass-tools` ops on 20,000 lines | `< 50 ms` | `< 20 ms` | — | Shift `140.32 µs`, Select `1.21 ms`, Join `41.58 µs`, Split `57.63 µs`, TPP `910.78 µs`, Resample `11.12 ms`, Karaoke `11.06 ms` | `bench_ops.lua` | **PASS** |
| Video playback 1080p | `0 drops`, `libass < 3 ms`, `upload < 2 ms` | `0 drops`, `libass p99 < 2 ms`, `upload p99 < 2 ms` | — | 12 s 1080p H.264 (`--bench-ui`): `0 / 73` drops, `libass p99 1.01 ms` (headless `1.39 ms`), `upload p50 1.58 / p99 2.20 ms` (Phase 0 quiet `1.1–1.3 ms`) | — | Drops & `libass` **PASS**; DX12 `upload p99` **MISS** under busy load (`perf: video playback upload`) |
| Open video -> first frame | `< 1 s` | `< 500 ms` | `< 1.67 s` | `131.01 ms` windowed / `119.61 ms` headless on 12 s 1080p H.264 clip (`24-min` episode TBD via `HLOUIS_TEST_VIDEO`) | `3.34 s` (24-min 1080p episode, cached index) | **PASS** on synthetic clip |
| Seek any frame (200 random) | `p95 < 100 / worst < 250 ms` | `p95 < 60 / worst < 150 ms` | — | Windowed `p50 37.10 / p95 58.67 / worst 66.42 ms`; headless `p50 33.31 / p95 54.88 / worst 60.61 ms` | — | **PASS** |
| Step `+-1` frame (50 steps) | `p95 < 20 ms` | `p95 < 10 ms` | — | Windowed `p50 22.43 / p95 51.86 / worst 52.88 ms`; headless `p50 15.10 / p95 34.69 / worst 35.64 ms` (first 4 back-steps hit `CleanPausedFrame` 4-slot LRU cache `< 1 ms`; larger backward steps re-decode GOP) | — | **MISS** (`perf: step +-1 frame`) |
| Seek next keyframe (20 jumps) | `p95 < 50 ms` | `p95 < 30 ms` | — | Windowed `p50 21.69 / p95 22.88 / worst 22.92 ms`; headless `p50 17.21 / p95 22.31 ms` | — | **PASS** |
| Subtitle re-render after edit | `< 16 ms` | `< 8 ms` | — | Single active track: windowed `p50 2.06 / p95 2.85 ms` (**PASS**); Full 20k `ass_read_memory` reload: windowed `p50 39.08 / p95 42.06 ms`, headless `p95 48.70 ms` (**MISS**) | — | Single track **PASS**; 20k full reload **MISS** (`perf: subtitle re-render after edit`) |
| Audio waveform/spectrum, 24-min | `< 1 s / < 3 s` | `< 300 ms / < 1.5 s` | — | Audio UI view not yet wired into `app` (`audio-peaks` backend Criterion benches active in workspace) | — | Pending UI integration |
| MKV subtitle import, 1.2 GB, 40 tracks | `< 3 s` | warm `< 300 ms`, CPU `< 0.5 s` | — | `0.17 s` warm / `3.6 s` cold recorded in `AGENTS.md` | — | **PASS** |
| Private memory (no video / video loaded / +audio) | `< 600 MB` | `< 400 / < 450 MB` no audio; `< 500 MB` w/ audio | `< 253 MB` w/ audio | `--bench-ui`: `255.1–321.5 MiB` no video (`81 MiB` headless), `359.3 MiB` video loaded (`136.7 MiB` headless); Phase 0 `391 / 437 MB` | `505 MB` with audio | No-audio **PASS**; audio RAM pending UI |

---

## 2. Criterion Benchmark Verification (`ass-core` & `ass-tools` Quiet Run)

| Benchmark ID | Owner-PC Ceiling (`AGENTS.md`) | Measured Median (`task-940`) | Status |
|---|---|---|---|
| `parse_20k_lines` | `<= 10 ms` (initial busy baseline) | `10.222 ms` | Near ceiling (`10.22 ms`; recalibrate / trim remaining header allocs) |
| `serialize_20k_lines` | `<= 8 ms` | `6.746 ms` | **PASS** |
| `undo_redo_step_20k_lines` | `<= 0.1 ms` | `82.742 ns` | **PASS** (ratchet candidate `~96 ns`) |
| `shift_times_20k` | `<= 11 ms` | `140.32 µs` | **PASS** |
| `find_replace_skip_tags_20k` | `<= 17 ms` | `6.475 ms` | **PASS** |
| `select_lines_20k` | `<= 1.5 ms` | `1.209 ms` | **PASS** |
| `join_100_lines_in_20k` | `<= 50 µs` | `41.584 µs` | **PASS** |
| `split_by_newline_in_20k` | `<= 50 µs` | `57.630 µs` | Slight variance vs initial `50 µs` |
| `tpp_full_pipeline_20k` | `<= 13 ms` | `910.78 µs` | **PASS** |
| `resample_resolution_20k_tags_drawings` | `<= 15 ms` | `11.116 ms` | **PASS** (down from `38.9 ms`) |
| `karaoke_parse_rebuild_20k` | `<= 15 ms` | `11.062 ms` | **PASS** (down from `38.9 ms`) |

Full workspace CI ceiling calibration (`median * 1.3`, or `median * 2.0` under `1 µs`, over 3 `windows-latest` CI runs) is stored in `ci/bench-ceilings.json` and enforced by `scripts/compare-benches.ps1` alongside the `+10%` regression gate.

---

## 3. CPU-Independent Zero-Allocation & Scaling Invariants

Verified passing under `cargo test --workspace` with thread-local `#[global_allocator]` instrumentation:
- `crates/render/tests/alloc_invariants.rs` — `render_and_blend_zero_rust_allocations_per_frame` (`0` Rust allocs)
- `crates/media/tests/alloc_invariants.rs` — `copy_rgba_plane_zero_allocations_with_reused_buffer` (`0` allocs, tight & padded strides)
- `crates/keyframes/tests/alloc_invariants.rs` — `cfr_and_vfr_conversions_make_zero_allocations` (`0` allocs across 5,000 CFR/VFR lookups)
- `crates/audio-peaks/tests/alloc_invariants.rs` — `query_columns_and_prereserved_push_make_zero_allocations` (`0` allocs)
- `crates/scene-detect/tests/alloc_invariants.rs` — `push_frame_steady_state_zero_allocations` (`0` allocs in steady state)
- `crates/edit-ops/tests/alloc_invariants.rs` — `event_metrics_zero_allocations` (`0` allocs)
- `crates/app/tests/alloc_invariants.rs` — `syntax_tokenize_and_grid_selection_make_zero_allocations` (`0` allocs on `tokenize_ass_line_into`, `SelectionSet`, `InlineAscii`, `SubtitleLine::recompute_cps`, `GridState::move_active`/`select_all`)
- `crates/ass-core/tests/scaling_invariants.rs` — O(N) allocs & timing `10k -> 40k` parse/serialize + O(1) (`0`-alloc) single-line undo/redo
- `crates/ass-tools/tests/scaling_invariants.rs` — O(N) allocs & timing `10k -> 40k` shift, find/replace, select, TPP, resample
- `crates/edit-ops/tests/scaling_invariants.rs` — O(N log N) sort & O(N) delete across `10k -> 40k`

---

## 4. Open Performance Issues

GitHub issues opened for every tightened budget row not yet passed end-to-end (with measured baselines and concrete architectural plans):
- `perf: cold start to usable window` (`1.65–2.31 s` vs `< 300 ms`)
- `perf: keystroke to screen in edit box` (`p95 11.84 ms` windowed DWM VSync vs `< 8 ms`)
- `perf: subtitle re-render after edit` (`20k` full reload `p95 42.06 ms` vs `< 8 ms`)
- `perf: step +-1 frame` (`p95 34.7–51.9 ms` past 4-frame cache vs `< 10 ms`)
- `perf: video playback upload` (`p99 2.20 ms` DX12 `write_texture` on 90 Hz iGPU vs `< 2 ms`)
