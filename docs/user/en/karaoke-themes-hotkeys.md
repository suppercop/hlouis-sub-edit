# Karaoke Templater, Themes / Eye Comfort & Hotkeys

## Karaoke Templater & Template Doctor

- **Template Doctor** (`Automation → Template Doctor`): Lints ASS karaoke templates and Lua 5.1 expressions before execution, highlighting syntax issues, unknown variables, and common pitfalls, with one-click idempotent safe fixes.
- **Karaoke Templater**: Runs Aegisub 3.5.1-compatible `code`, `template`, and `mixin` lines (`once`, `line`, `syl`, `char`, `furi`, `noblank`, `notext`, `keeptags`, `loop N`, `fxgroup`, `all`, `retime()`, `relayer()`, `restyle()`, `maxloop()`, `$x`/`$y`/`$sstart`/`$send` inline variables, `!expr!` expressions) inside a sandboxed LuaJIT VM (`io`, `os`, `package.loadlib` blocked; memory and instruction limits enforced).
- Generated lines are tagged with `Effect = fx` and can be stripped and regenerated cleanly at any time.

## Themes & Eye Comfort Mode

Switch themes live from the top bar or **Preferences → Interface / Colours**:
- **Light** and **Dark** themes calibrated for long subtitling sessions.
- **Eye Comfort**: Warm low-blue-light tint with a **0–100% strength slider** and optional evening schedule.
- **Language**: Toggle **EN / VI** live without restarting.

## Hotkeys & Medusa Numpad Set

- Context-aware dispatch matches Aegisub's priority order: `Always` → focused panel (`Subtitle Grid`, `Subtitle Edit Box`, `Audio`, `Video`, `Styling Assistant`) → `Default`.
- Import your existing Aegisub 3.5.1 `hotkey.json` (including the **Medusa numpad** bindings `KP_0`–`KP_9`, `KP_Enter`, `KP_Add`, `KP_Subtract`) and `config.json` from **Preferences → General → Import my Aegisub settings...**.

## Crash Recovery & Diagnostics

- **Atomic Autosave**: Every autosave writes to a temporary file, calls `fsync`, and atomically renames—never corrupting your script on disk if power is lost.
- **Crash Recovery**: After an unclean shutdown, HLouis Sub Edit lists recoverable scripts with timestamps, line counts, and a diff preview against the file on disk.
- **Event Log**: Filter logs by severity or module and export a privacy-safe Problem Report bundle (personal file paths redacted by default).
