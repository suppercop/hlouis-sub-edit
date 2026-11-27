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
