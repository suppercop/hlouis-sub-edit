# Phase 0 brief — Spike & foundation (2–3 weeks)

**Goal:** prove the chosen stack (Rust + egui/eframe on wgpu + libass + FFMS2 + Fluent) can handle the hardest parts of the app on Windows 10/11 *before* real development starts. This is throwaway-quality UI on top of production-quality project setup.

Branch: `phase-0`. Finish with a PR to `main` and `docs/phase-0-report.md`.

## Tasks

### T1. Workspace & tooling
- Cargo workspace per `AGENTS.md` layout. Create only: `crates/app`, `crates/i18n`, `crates/libass-sys`, `crates/ffms2-sys`, `crates/render`, `crates/media` (minimal).
- `rust-toolchain.toml` (stable, pinned), `rustfmt.toml`, workspace `[lints]` with clippy pedantic warnings allowed only where justified.
- Native libs: get **libass** and **FFMS2** for Windows x64 via vcpkg manifest (`vcpkg.json`) or pinned prebuilt downloads with SHA-256 check, in a script `scripts/fetch-native.ps1`. Nothing binary committed.
- `docs/THIRD_PARTY.md` listing each native lib, version, license, how it is linked.

### T2. CI
- GitHub Actions on `windows-latest`: fetch native libs (cached), `fmt --check`, `clippy -D warnings`, `test`, `build --release`, upload the release build (exe + DLLs) as an artifact.

### T3. Grid spike
- Virtualised table with **50,000 generated lines**, columns `#`, Start, End, CPS, Style, Text (Text may contain `{\tags}`).
- Smooth scrolling with mouse wheel and touchpad, click / Shift+click / Ctrl+click multi-select, keyboard up/down.
- Show an FPS / frame-time overlay.

### T4. Edit box spike
- Multiline text box bound to the selected grid line.
- Syntax highlighting with `LayoutJob`: override blocks `{...}`, tag names, values, `\N`/`\n`/`\h`, karaoke tags, drawing commands — colours come from the theme.
- **IME:** must work with (a) Windows built-in Vietnamese Telex keyboard, (b) UniKey or EVKey in Telex, (c) Microsoft Japanese IME. Correct composition, no doubled/lost characters, candidate window near the caret.

### T5. Video + libass spike
- Open a video via FFMS2 (index with progress), decode to RGBA, render an `.ass` file with libass at the frame time, alpha-blend, upload to a `wgpu` texture shown in an egui panel.
- Play at native rate (test 1080p 23.976 H.264 and 1080p HEVC 10-bit), pause, seek to frame N, next/prev frame, jump to next/prev keyframe (keyframes from FFMS2).
- Overlay with decode ms, render ms, upload ms, dropped frames.
- Window resizes and High-DPI scaling (125%, 150%) keep the video crisp and correctly scaled.

### T6. Theme & i18n spike
- Theme setting Light / Dark / Follow Windows, switchable at runtime; grid, edit-box highlight colours and overlay follow it.
- Fluent `en` and `vi` resources for all spike strings, switchable at runtime; Vietnamese diacritics render correctly in all widgets.

### T7. Report
`docs/phase-0-report.md` with: build/run steps on clean Windows 11, measurements table (grid FPS while scrolling 50k lines, memory, video ms per stage at 1080p, dropped frames over 60 s playback), IME test results per input method, screenshots light/dark/vi, open issues, and a clear **recommendation: continue with egui, or switch to Qt (cxx-qt)**, with reasons.

## Acceptance criteria (Claude will verify each one)

| # | Criterion |
|---|---|
| A1 | CI green on `windows-latest`; artifact runs on a clean Windows 11 machine with only the bundled DLLs |
| A2 | `fmt`, `clippy -D warnings`, `test` pass; no `unsafe` outside `*-sys`/`ffi`, all with `// SAFETY:` |
| A3 | No binaries committed; `THIRD_PARTY.md` complete; no GPL code linked |
| A4 | 50k-line grid scrolls at ≥ 60 FPS on a mid-range PC, multi-select works |
| A5 | Vietnamese (built-in Telex + UniKey/EVKey) and Japanese IME type correctly in the edit box |
| A6 | 1080p 23.976 H.264 plays with libass subtitles, < 1% dropped frames over 60 s; frame-accurate seek |
| A7 | Light/Dark/System and en/vi switch at runtime without restart |
| A8 | High-DPI 125%/150% renders correctly |
| A9 | Report complete with numbers and an egui-vs-Qt recommendation |

## Not in this phase
Real ASS parsing (a naive line split is fine for the spike), audio, styles, saving files.
