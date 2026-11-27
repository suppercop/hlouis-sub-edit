# Plan (summary)

Full plan with rationale is kept by the project owner. This file is the scope agents must follow.

## In scope
- **Files:** full ASS (Script Info, Styles, Events, embedded Fonts/Graphics, Aegisub Project Garbage), SRT read/write, read subtitles from MKV (incl. compressed), charset detection (BOM, UTF-8, uchardet-like), autosave + autobackup + crash recovery, project links to audio/video/keyframes/timecodes.
- **Grid:** all columns, show/hide columns, multi-select, copy/cut/paste/paste-over, insert/duplicate/delete, undo/redo, join/split, swap/move, comment, sort, state colouring, CPS with warning colours, tag display modes, smooth touchpad scroll, configurable font size.
- **Edit box:** all fields, syntax highlighting (tags, karaoke, drawings/vector clips), B/I/U/S/font/colour buttons, time vs frame mode, Show Original, `\h` hotkey, Shift+Enter `\n` option, style/actor autocomplete, char count, IME.
- **Audio/timing:** load from file or video, cache, waveform, spectrum, zoom/scroll, selection & markers, commit, all play modes, keyframes on audio + snapping, snap to neighbours, inactive lines display, lead-in/out, auto-commit/next, auto-scroll, new video-position marker with frame-duration areas, scroll-to-line-start/end commands, karaoke timing mode. Playback via `cpal` (WASAPI).
- **Video/render:** FFMS2, Avisynth, dummy video, libass rendering, seek by frame/time/keyframe/line, play with audio, zoom & aspect ratio, pan/zoom, detach window, VFR timecodes, rotated/flipped video, video details incl. colour space, YCbCr Matrix handling & prompts, screen colour picker, save/copy frame, save/copy transparent subtitle image, webm/h264/hevc/opus/eac3.
- **Keyframes:** load/save external keyframe files (XviD pass log, x264/x265 stats, Aegisub keyframe format), keyframes from video, **SCXvid detection inside the app** (background, progress, cancel, cached result).
- **Styles:** manager, storage, import, full editor, preview, border style 4 / encoding -1, rename propagates, colour picker.
- **Script properties & resample:** all Script Info fields, PlayRes, LayoutRes, WrapStyle, ScaledBorderAndShadow, YCbCr Matrix, sync prompts, Resample Resolution.
- **Visual tools:** drag, rotate Z, rotate X/Y, scale, rect clip, vector clip (all modes, hotkeys), cross, configurable colours, Align to Video.
- **Tools:** Find & Replace, Select Lines, Shift Times, Timing Post-Processor, framerate transform on export, snap to video frame.
- **Karaoke:** `\k` tags, karaoke audio mode, Karaoke Templater (user-written templates) via LuaJIT + karaskel + bundled helper libs.
- **Fonts:** check for missing fonts only.
- **UI:** context hotkeys, toolbar/menu, layouts, preferences, Light/Dark/System theme, English + Vietnamese, High-DPI, log window, version check, scroll-modifier options.
- **Providers from arch1t3cht fork:** BestSource, VapourSynth.

## Known Aegisub bug we must not repeat: MKV with many subtitle tracks
Aegisub 3.5.1 "Open Subtitles from Video" silently returns an empty script for MKVs with >32 tracks (owner's file: 40 tracks, 37 SRT subtitle tracks; likely the 32-bit track mask in Haali's MatroskaParser). Phase 1 requirements:
- No track-count limit; demux via libavformat (or a Matroska parser without track limits).
- Track picker shows index, codec, language, name, default/forced flags and a preview of the first lines; allow importing several tracks at once.
- Read only the selected track(s), in the background with progress + cancel; the owner's 40-track 1080p episode must finish in ≤ 3 s.
- Never fail silently: a track yielding 0 events reports why.
- Regression tests: the owner's file (path via env var, not committed) and a generated MKV with ≥ 64 subtitle tracks (ASS, SRT, zlib-compressed, header-stripped).

## UI direction (owner preference)
The owner prefers the **classic Aegisub 3.2.2 look and layout** over the 3.4/3.5 UI, and has tuned their own Aegisub 3.5.1 to look like 3.2.2. Use that as the reference: same panel arrangement (video top-left, audio top-right, edit box under audio, grid at the bottom), compact toolbar, classic grid/edit-box density and colours as the Light theme default. New features (pan/zoom, new audio marker, dark theme) are added without changing that layout. The owner's Aegisub config and hotkeys (%APPDATA%\Aegisub\config.json, hotkey.json) are the reference for default layout options and hotkeys; import them where they map.

## Out of scope
SSA, TTXT, MicroDVD, plain text, EBU STL, Encore, Transtation; open-with-charset; recent files; Export As filters; attachments manager; VSFilter; spell check, thesaurus, translation assistant; Kanji timer; MoonScript, automation manager, macro menu, Lua export filters, `aegisub.*` macro API, DependencyControl; font copy/zip; other UI languages; line folding; perspective tool; Linux/macOS.

## Phases
0. Spike & foundation (2–3 w) — see `docs/phase-0.md`
1. Core data: ASS/SRT round-trip, charset, MKV subs, undo, autosave (3–4 w)
2. Basic editor: grid, edit box, styles, script props, find/replace, select; i18n + theme framework (4–6 w) — Milestone A
3. Audio & timing (5–7 w) — Milestone B
4. Video & keyframes incl. SCXvid; **BestSource as default video/audio provider, FFMS2 as fast-open fallback**, both behind the common `media` trait (6–8 w) — Milestone C
5. Typesetting tools, resample, Align to Video (5–6 w)
6. Karaoke Templater (2–3 w)
7. VapourSynth/Avisynth, fonts check, prefs/hotkeys polish, Windows installer (3–5 w)
