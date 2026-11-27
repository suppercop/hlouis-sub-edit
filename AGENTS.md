# AGENTS.md — rules for any AI coding agent working in this repo

This file is the contract for whoever writes code here (currently Gemini; Claude reviews and accepts each phase).
Read it fully before every task. Also read `docs/PLAN.md` and the brief for the current phase in `docs/`.

## Project

`hlouis-sub-edit` is a subtitle editor in the spirit of Aegisub 3.5.1, rewritten in Rust.
Scope is fixed in `docs/PLAN.md`. Do not add features that are not in the plan; do not drop features that are.

- Target platform: **Windows 10/11 x64 only.** No Linux/macOS code paths required.
- UI languages: **English and Vietnamese** (Fluent `.ftl` files). No hard-coded user-visible strings.
- Themes: **Light, Dark, Follow Windows, Eye comfort.** Every colour in the UI is a theme token and must pass the contrast/blue-light tests of `docs/eye-comfort.md`; no blur/overlay/dimming tricks.
- GUI: `egui`/`eframe` on the `wgpu` backend (may change to Qt only if Phase 0 says so).

## Performance is a hard requirement

The whole reason for this rewrite is speed and smoothness. A feature that works but misses its budget is **not done**.

### Owner's performance rules
- **Goal — "Aegisub, optimised in Rust":** EVERY metric must be clearly better than Aegisub 3.5.1 measured on the same PC and files (`docs/aegisub-baseline.md`, `docs/perf-report.md`) — as far below Aegisub as possible. Target: **at most HALF of Aegisub's number** on every row where Aegisub is measured (time, CPU, memory, latency), and never worse on any row. Where the table below is looser than half of Aegisub, the half-of-Aegisub number wins (e.g. memory with audio `< 253 MB` is the goal, `< 505 MB` is the hard fail line).
- **Budgets are CEILINGS, not targets.** Keep optimising while there is a cheap win; stop only at diminishing returns and say why.
- **Ratchet:** when a metric improves, lower its ceiling to `(new measured value + 15%)` in the same PR, so speed never regresses.
- **Reproducible Aegisub baseline:** for rows where Aegisub is not measured yet (startup to usable, keystroke latency, grid scroll, seek, frame step, re-render, waveform, open/save 20k lines, find-replace, every tool), keep reproducible benchmarks in `tools/aegisub-bench/` (Aegisub log timestamps, UI Automation key->repaint timing, or Automation Lua macros on the same 20k-line file) and record the Aegisub column in `docs/perf-report.md`.

### Budgets (release build, Windows 11, mid-range laptop, 1080p display at any DPI)

Every metric is **p95** unless it says `worst` or `avg`. "Aegisub" means Aegisub 3.5.1 measured on the same PC and files (`docs/aegisub-baseline.md`, `docs/perf-report.md`), and we must beat it on every row where it was measured.

| Area | New budget (old) | Measured so far |
|---|---|---|
| Idle, paused, no input | CPU < 0.2% of one core, 0 repaints/s (was < 0.5%) | 0.10-0.31%; Aegisub 0.31% |
| Cold start to usable window (no media) | < 300 ms (was 500) | not measured; Aegisub 6.35 s |
| Open / save 20,000-line ASS (incl. charset, I/O, grid model) | < 40 ms / < 25 ms (was 10k < 150 / < 100) | parse 7.2 ms, serialize 5.2 ms |
| Grid scroll 200,000 lines | every frame at display refresh up to 120 Hz, CPU frame time worst < 8 ms; select-all < 10 ms (was 16 / 50) | - |
| Keystroke to screen in edit box (highlight + grid refresh + re-render) | < 8 ms (was 16) | - |
| Undo/redo 20,000 lines / find-replace 20,000 lines | < 1 ms / < 30 ms (was 5 / 200) | 0.06 ms / 8.6 ms |
| Every ass-tools operation on 20,000 lines (shift, select, join/split, TPP, resample, karaoke) | < 20 ms (was 50) | <= 10.4 ms |
| Video playback 1080p H.264, HEVC 10-bit, 1080p60 with subs | 0 dropped over 60 s; libass p99 < 2 ms/frame, upload p99 < 2 ms (was libass < 3) | 0 drops, libass 0.06-0.91, upload 1.1-1.3 |
| Open 24-min 1080p episode, index cached | first frame < 500 ms (was 1 s) | Aegisub 3.34 s |
| Seek any frame / +-1 frame / next keyframe | < 60 ms (worst < 150) / < 10 ms / < 30 ms (was 100/250, 20, 50) | decode ~7 ms, ~43 ms after long seek |
| Subtitle re-render after an edit (current frame) | < 8 ms (was 16) | - |
| Audio waveform/spectrum, 24-min episode | first view < 300 ms, full < 1.5 s background (was 1 s / 3 s) | - |
| MKV subtitle import, 1.2 GB, 40 tracks | warm cache < 300 ms; cold limited only by disk read, CPU < 0.5 s (was < 3 s) | 0.17 s warm, 3.6 s cold |
| Private memory, 1080p episode + 20,000 lines, paused / playing | < 400 MB / < 450 MB without audio; < 500 MB with audio, i.e. below Aegisub's 505 MB (was 600) | 391 / 437 MB |

### Measurement method & ceiling sets

Because the owner's PC is often busy with background builds/agents, timing numbers are split into two independent ceiling sets plus CPU-independent invariant checks:

1. **CI ceilings (GitHub `windows-latest` runner):** calibrate by running every Criterion bench 3 times in CI; `CI ceiling = median × 1.3` (`× 2.0` for micro-benchmarks under `1 µs`, which are noise-dominated). CI enforces both the `> 10%` Criterion regression gate against `main` and these absolute CI ceilings.
2. **Owner-PC acceptance numbers:** measured only when no agent or build is running, on AC power, across 3 runs (reporting the median). User-facing budgets in the table above stay as hard limits; bench ceilings only ever move downward (ratchet to `new measured + 15%` after a quiet improvement).
3. **CPU-independent invariant checks (in `cargo test`):**
   - **Allocation count:** counting global allocator tests assert `0` heap allocations on per-frame / per-keystroke / per-mouse-move hot paths (`render`, `media::copy_rgba_plane`, `keyframes`, `audio-peaks::query_columns`, `scene-detect`, `edit-ops::event_metrics`, `app::syntax` / `app::grid` selection & cached layout paths).
   - **Scaling tests:** verify `n = 10k` vs `20k` vs `40k` stays ~linear (`O(N)` / `O(N log N)`) for full-script operations and ~constant (`O(1)`) for incremental updates / single-line undo/redo.

#### Initial owner-PC bench ceilings (ratchet starting point)
Measured while busy; lower after a quiet run (ratchet `+15%`):
- `ass-core`: parse 20k `<= 10 ms`, serialize 20k `<= 8 ms`, undo/redo 20k `<= 0.1 ms`
- `ass-tools`: shift 20k `<= 11 ms`, select 20k `<= 1.5 ms`, find-replace 20k `<= 17 ms`, TPP 20k `<= 13 ms`, join/split `<= 50 µs`, resample 20k `<= 15 ms`, karaoke parse+rebuild 20k `<= 15 ms`
- `keyframes`: 200k CFR `<= 6 ms`, 200k VFR `<= 5 ms`
- `fonts`: check 20k `<= 20 ms`
- `autosave`: handoff/end-to-end `<= 5 ms` (snapshot handoff) / `<= 30 ms` with fsync; `<= 5 ms` target
- `project`: read+write links 20k `<= 1 µs`
- `audio-peaks`: waveform full 24-min `<= 150 ms`, first screen 10s `<= 0.5 ms`, 2000-px query `<= 30 µs`, spectrum 24-min `<= 270 ms`
- `scene-detect`: 1080p quarter-res `<= 0.5 ms/frame`
- `edit-ops`: undoable sort 20k `<= 6 ms`, paste 5k `<= 4 ms`, delete 10k `<= 2 ms`, CPS 20k `<= 1.5 ms`
- `styles-catalog`: rename+propagate 20k `<= 0.8 ms`, import 200 styles `<= 130 µs`
- `aegisub-config`: parse+map `<= 50 µs`
- `mkv-subs`: 40-track warm 16 MB skip-heavy `<= 0.25 s` (1.2 GB warm cache `< 300 ms`)
- Phase 1b+ crates (applied as each crate lands): `syntax` tokenize `<= 20 µs` / re-tokenize `<= 1 µs` / grid display `<= 15 µs`; `edit-box` ops `<= 3 µs`; `timing` drag `<= 100 ns`; `karaoke` drag `<= 100 ns` / commit `<= 2 µs`; `audio-view` frame `<= 1.5 µs`; `visual-tools` drag `<= 1 µs` / clip serialize `<= 25 µs`; `align-to-video` `<= 0.7 ms`; `video-geometry` `<= 50 ns` / dummy frame `<= 0.35 ms`; `frame-tools` transform `<= 6 ms` / snap `<= 2.3 ms`

### Rules
1. Nothing slow on the UI thread: decoding, indexing, waveform/spectrum, file I/O, Lua, font scanning, SCXvid all run on worker threads with progress + cancel. The UI thread only lays out and paints.
2. No per-frame allocations in hot paths (video, audio view, grid rows): reuse buffers/pools; prefer borrowing over cloning strings.
3. Virtualise everything that scales with script length; never iterate all lines per frame.
4. Cache expensive results (indexes, waveform peaks, layout jobs, libass tracks) and invalidate precisely.
5. Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `opt-level = 3`. Target baseline x86-64-v2 (no `target-cpu=native` in shipped builds).
6. Every crate with hot code has `criterion` benches; CI runs them and **fails on a >10% regression** against `main` or if an absolute CI ceiling is exceeded.
7. Every phase report includes a performance table against these budgets **and a side-by-side with Aegisub 3.5.1** on the same machine and files. Missing a budget = open issue that blocks acceptance unless the owner waives it.
8. **Theme tokens & eye comfort:** every colour in the UI must be a theme token from the theme palette (no hardcoded colours elsewhere) and pass the contrast and blue-light radiance tests in `docs/eye-comfort.md`; never use blur, haze, or dimming overlays.

## Repository layout (Cargo workspace)

```
crates/
  app/          # binary: GUI, hotkeys, preferences, theme, i18n wiring
  ass-core/     # ASS/SRT data model, parser/serializer, override-tag parser, undo/redo
  ass-tools/    # shift times, TPP, resample, join/split, find/replace, select lines
  media/        # video/audio provider traits + FFMS2/BestSource/VapourSynth/Avisynth impls
  keyframes/    # keyframe file I/O, SCXvid detection
  render/       # libass rendering, compositing
  audio-view/   # waveform/spectrum computation, playback (cpal)
  visual/       # visual typesetting tools geometry
  scripting/    # LuaJIT (mlua) for karaskel + Karaoke Templater only
  fonts/        # missing-font checker
  i18n/         # Fluent loader, en + vi resources
  *-sys/        # raw FFI bindings (libass-sys, ffms2-sys, ...)
```
Only create crates when a phase needs them.

## Hard rules

1. Rust stable, edition 2021. Pin the toolchain in `rust-toolchain.toml`.
2. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` must pass on `windows-latest` CI before you open a PR.
3. `unsafe` only inside `*-sys` crates or a single clearly named `ffi` module per crate. Every `unsafe` block gets a `// SAFETY:` comment.
4. No `unwrap()`/`expect()` in library crates except in tests. Use `thiserror` for library errors, `anyhow` only in `app`.
5. Core logic (`ass-core`, `ass-tools`, `keyframes` parsing) must not depend on any GUI crate and must have unit tests.
6. **Never commit binaries**: no DLLs, videos, large samples, build output. Fetch native libs in CI/scripts (vcpkg or pinned downloads with checksums). Put test media paths behind env vars.
7. **Licensing:** project is BSD-3-Clause. Do not link GPL code (xvidcore, SCXvid) into our binaries; run it out-of-process or through VapourSynth. LGPL libs only via dynamic linking. List every native dependency and its license in `docs/THIRD_PARTY.md`.
8. Do not copy code from Aegisub without keeping its copyright notice in the file header.
9. One branch per phase (`phase-0`, `phase-1`, ...). Small, meaningful commits. Open a PR to `main` when the phase is done; never push directly to `main`.
10. If a requirement in the brief is impossible or unclear, do not silently skip it: write it under "Open issues" in the phase report.

## Definition of done for every phase

- All acceptance criteria in the phase brief are met, or listed as open issues with reasons.
- `docs/phase-N-report.md` written: what was built, how to run it, measurements, open issues, deviations from plan.
- PR description links the report and gives exact steps to build and run on a clean Windows 11 machine.
