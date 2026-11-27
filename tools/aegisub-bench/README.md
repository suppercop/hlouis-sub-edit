# Aegisub 3.5.1 Reproducible Benchmark Harness

Reproducible comparison suite measuring **Aegisub 3.5.1** on the same Windows 11 PC and 20,000-line ASS script used by `hlouis-sub-edit --bench-ui` (`crates/app/src/bench_ui.rs`).

## Files

- `parse-aegisub-logs.ps1` — Read-only parser for `%APPDATA%\Aegisub\log\*.json`. Computes startup (`agi/options` -> `Leaving FrameMain constructor`), `Open` / `Save` / `libass` / `ffms2cache` / `DirectSound` timings from Aegisub's nanosecond-resolution log events (`sec * 1e9 + usec`). **Never modifies `%APPDATA%\Aegisub`.**
- `bench_ops.lua` — Automation 4 Lua macro (`HLouis Benchmark (20k Ops)`) using LuaJIT FFI `QueryPerformanceCounter` (fallback `os.clock()`) to measure 20,000-line script traversal, Shift Times, Find/Replace (skipping `{...}` tags), Select matching lines, Karaoke syllable parse/rebuild, CPS scan, and Undo commit. Writes `%TEMP%\aegisub_lua_bench.json`.
- `run-aegisub-bench.ps1` — Full runner:
  - Default mode (`-ParseLogsOnly`): parses existing Aegisub logs read-only and generates `$env:TEMP\aegisub-bench\bench_20k.ass` without launching Aegisub or altering user settings/MRU.
  - `-RunInteractive`: generates the identical 20k ASS script, launches `C:\Program Files\Aegisub\aegisub.exe`, measures cold start via UI Automation (`WaitForInputIdle` + UIA `WindowPattern` + Aegisub JSON log diff), samples keystroke-to-repaint latency (Edit box `SendInput` + DWM/GDI pixel-change polling), 200k/20k grid scroll timing, and prints a comparison table alongside `hlouis-sub-edit --bench-ui`.

## Safe Usage

```powershell
# 1. Read-only log extraction + 20k ASS fixture generation (zero writes to %APPDATA%\Aegisub)
powershell -ExecutionPolicy Bypass -File tools\aegisub-bench\run-aegisub-bench.ps1

# 2. Full interactive owner-PC measurement (close Aegisub first; ensure AC power, idle PC)
#    Optional: copy bench_ops.lua into Aegisub's automation\autoload folder first to time Lua macros.
powershell -ExecutionPolicy Bypass -File tools\aegisub-bench\run-aegisub-bench.ps1 -RunInteractive -MeasureSideBySide
```
