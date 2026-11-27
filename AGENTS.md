# AGENTS.md — rules for any AI coding agent working in this repo

This file is the contract for whoever writes code here (currently Gemini; Claude reviews and accepts each phase).
Read it fully before every task. Also read `docs/PLAN.md` and the brief for the current phase in `docs/`.

## Project

`hlouis-sub-edit` is a subtitle editor in the spirit of Aegisub 3.5.1, rewritten in Rust.
Scope is fixed in `docs/PLAN.md`. Do not add features that are not in the plan; do not drop features that are.

- Target platform: **Windows 10/11 x64 only.** No Linux/macOS code paths required.
- UI languages: **English and Vietnamese** (Fluent `.ftl` files). No hard-coded user-visible strings.
- Themes: **Light, Dark, Follow Windows.** No hard-coded colors outside the theme module.
- GUI: `egui`/`eframe` on the `wgpu` backend (may change to Qt only if Phase 0 says so).

## Performance is a hard requirement

The whole reason for this rewrite is speed and smoothness. A feature that works but misses its budget is **not done**.

### Budgets (release build, Windows 11, mid-range laptop, 1080p display at any DPI)
| Area | Budget |
|---|---|
| Idle (paused, no input) | CPU < 0.5% of one core; repaint only on input, playback or new results |
| Cold start to usable window (no media) | < 500 ms |
| Open 10,000-line ASS / save it | < 150 ms / < 100 ms |
| Grid scroll, 200,000 lines | never below display refresh (60/90/120 Hz); no frame > 16 ms; select-all < 50 ms |
| Keystroke to screen in edit box (incl. highlight + grid refresh) | < 16 ms |
| Undo/redo / find-replace on 20,000 lines | < 5 ms / < 200 ms |
| Video 1080p H.264, HEVC 10-bit and 1080p60 playback with subs | 0 dropped frames over 60 s; libass < 3 ms/frame |
| Open a 24-min 1080p episode (index cached on disk) | first frame < 1 s; uncached indexing runs in background with progress |
| Video seek to any frame / ±1 frame / next keyframe | < 100 ms (worst < 250 ms) / < 20 ms / < 50 ms |
| Subtitle re-render after an edit (current frame) | < 16 ms |
| Audio waveform for a 24-min episode | first view < 1 s, full < 3 s (background) |
| Audio display scroll/zoom (waveform or spectrum) | at display refresh |
| Memory (private bytes), 1080p episode + 20,000-line script | < 600 MB |

### Rules
1. Nothing slow on the UI thread: decoding, indexing, waveform/spectrum, file I/O, Lua, font scanning, SCXvid all run on worker threads with progress + cancel. The UI thread only lays out and paints.
2. No per-frame allocations in hot paths (video, audio view, grid rows): reuse buffers/pools; prefer borrowing over cloning strings.
3. Virtualise everything that scales with script length; never iterate all lines per frame.
4. Cache expensive results (indexes, waveform peaks, layout jobs, libass tracks) and invalidate precisely.
5. Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `opt-level = 3`. Target baseline x86-64-v2 (no `target-cpu=native` in shipped builds).
6. Every crate with hot code has `criterion` benches; CI runs them and **fails on a >10% regression** against `main`.
7. Every phase report includes a performance table against these budgets **and a side-by-side with Aegisub 3.5.1** on the same machine and files. Missing a budget = open issue that blocks acceptance unless the owner waives it.

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

## Never-silent errors (hard rule, owner decision 2026-11-27)

- Every import / export / open / save / autosave / recovery path returns a **typed error with a human-readable reason** (English text in the error's `Display` now; en + vi via `i18n-catalog` once it exists). The UI must show it; never swallow it.
- A function that can "succeed" with **0 items** (MKV track with 0 events, empty keyframe file, font check with nothing found, ...) must say so explicitly: return a typed error, or a report/warning value the UI shows.
- Forbidden on parse/IO results unless justified in a comment: `unwrap_or_default()`, `unwrap_or(..)` fallbacks, `let _ =` on a `Result`, `.ok()` that discards the error, returning an empty `Vec`/`None` on failure without a reason. Values that are present but invalid are reported (warning list), not replaced silently.
- Every new such path gets a test that asserts the error/warning is produced.
- Every new parser gets a fuzz target in `fuzz/` (cargo-fuzz); CI fuzzing is added separately.
- Timing-affecting changes must keep `frame-check` (see `crates/frame-check`) clean: the event's first/last visible frame according to our timing code must match what libass renders.

## Definition of done for every phase

- All acceptance criteria in the phase brief are met, or listed as open issues with reasons.
- `docs/phase-N-report.md` written: what was built, how to run it, measurements, open issues, deviations from plan.
- PR description links the report and gives exact steps to build and run on a clean Windows 11 machine.
