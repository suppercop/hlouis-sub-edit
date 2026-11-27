# UI reference — owner's Aegisub setup (classic 3.2.2 style)

Source: owner's Aegisub 3.5.1 `config.json` / `hotkey.json`, tuned to look like 3.2.2. Use these as **defaults** in Phase 2+ unless the owner says otherwise. Window/panel arrangement is not stored in those files; screenshots will be added here.

## General
- UI language English by default (Vietnamese available), maximised window, toolbar with 16 px icons, call tips off.
- Classic **light** palette is the default theme (dark theme available but not default).

## Grid
- Font size 9.
- Override tags hidden and replaced by the `☀` symbol.
- Lines visible on the current video frame get a highlighted background.
- Colours (classic): light-blue header, light-green left column, mint selection, light-purple comments, pink border on the active line.

## Edit box
- Font size 10.
- Light syntax colours: braces blue, backslash pink, tag names dark grey, parameters green.

## CPS / line length
- CPS warning at 15, error at 30; ignore punctuation and whitespace. Line length limit 40 characters.

## Audio (most visible 3.2.2 trait)
- Video-position marker and seconds ruler on the audio display **off** by default (3.5 added/enabled them; keep them as options).
- Waveform colour scheme "Green", spectrum "Icy Blue"; default view spectrum; display height 212 px.
- Medusa (numpad) timing keys; drag to adjust timing; auto-commit then go to next line; lead-in 100 ms / lead-out 350 ms; snapping on.

## Video
- Provider FFMS2 (auto thread count) — BestSource default from Phase 4 per plan; libass renderer; keyframes shown on the seek bar; visual tools auto-hide.

## Hotkeys
- Aegisub defaults, including Medusa keys and: ←/→ one frame, Shift+←/→ keyframe, Ctrl+←/→ line boundaries, Alt+←/→ large jump.

## Autosave / undo
- Autosave every 60 s and after each change, backups on, 50 undo levels.

## Panel layout (from owner's screenshot)
(The reference screenshot was removed before the repo went public because it contained a frame from a commercial episode; the layout is described below.)

- **Menu bar** (File, Edit, Subtitle, Timing, Video, Audio, Automation, View, Help) and **one row of 16 px toolbar icons** under it.
- **Top-left: video**, about half the window width, with the **visual tools as a vertical toolbar on its left edge**. Under the video: seek slider (with keyframe ticks), play / play line / pause / auto-scroll buttons, current time + frame box (`0:18:14.468 - 26241`), offset-to-line box (`+2068ms; -32ms`), zoom dropdown (`37.5%`).
- **Top-right: audio display** (spectrum view, timeline ruler on top, selection in red/magenta markers), with **three vertical sliders on the right** (horizontal zoom, vertical zoom, volume) and a lock toggle. One row of audio buttons under it (play controls, lead-in/out, commit, auto-commit/auto-next/auto-scroll toggles, spectrum/waveform toggle, karaoke).
- **Right, under audio: edit box.** Row 1: Comment checkbox, Style dropdown + Edit button, Actor, Effect, character count. Row 2: Layer spinner, Start, End, Duration, Margin L/R/V. Row 3: B, I, U, S, fn, four colour buttons, commit tick, Time/Frame radio, Show Original. Then the multiline text box filling the rest.
- **Bottom, full width: subtitle grid** with columns `#`, `L`, Start, End, CPS, Style, Effect, Text; light-blue header, green `#` column, comment lines tinted lavender, hidden tags shown as a symbol.
- The owner's scripts are heavy **karaoke-template** files (`code once`, `template pre-line/syl/char ... loop N`, `!retime(...)!`) — Templater fidelity and grid performance on long template lines matter.

## Owner decisions (2026-11-27)
- Panel proportions: default as in the screenshot (video ≈52% width, grid ≈1/3 height); panels are resizable and sizes are remembered between sessions.
- Grid columns Actor and Margin L/R/V hidden by default; re-enable via right-click on the grid header.
