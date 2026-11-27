# Editing, Timing, Video/Audio & Tools

## Opening Video & Subtitles (Including 40+ Track MKVs)

- Drag and drop `.ass`, `.srt`, `.mkv`, or keyframe/timecode files directly onto the main window, or use **File → Open Subtitles from Video...**.
- **Multi-Track MKV Importer**: Unlike Aegisub 3.5.1's 32-track ceiling, HLouis Sub Edit lists every subtitle track in an MKV (64+ tracks supported), displays codec (`ASS`, `SSA`, `SRT`, `WebVTT`, and disabled `PGS`/`VobSub` bitmap tracks with explanation), language, title, default/forced flags, line count, and a 5-line preview. You can filter by language/text and import multiple tracks either as separate documents or merged.

## Subtitle Grid & Edit Box

- **Per-row Edit Isolation**: Undo and cursor history are isolated per row, and rapid typing amends the active line with zero lag.
- **Time vs. Frame Mode**: Toggle between `H:MM:SS.cc` centisecond timestamps and frame numbers whenever video timecodes are loaded.
- **CPS Indicators**: The CPS column highlights high reading speeds so you can spot dense lines at a glance.

## Timing, Audio & Spectrum

- Switch between **Waveform** and **Spectrum** (`Regular`, `Better`, `High`, `Insane` FFT quality) from the Audio toolbar.
- Drag start/end timing handles directly on the audio display, snap to keyframes, or run **Timing → Timing Post-Processor (TPP)** and **Timing → Shift Times**.

## Styles & Native Tools

- **Styles Manager & Style Editor**: Edit `.sty` catalogs, import styles from another `.ass`, and propagate style renames across all dialogue lines.
- **Resample Resolution**: Scale coordinates, fonts, borders, shadows, and `\clip` drawings cleanly between resolutions (e.g. 720p ↔ 1080p).
- **HLouis Tools**:
  - **Select by Style** (`Ctrl+Alt+S` quick / `Ctrl+Alt+Shift+S` dialog): Select every line matching the active style or pick from style counts.
  - **Clean Whitespace & Redundant Line Breaks** (`Ctrl+Alt+W`): Normalizes Unicode spaces and collapses redundant `\N` breaks while safely skipping `\p1+` vector drawing lines, with a dry-run diff preview.
