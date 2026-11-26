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
