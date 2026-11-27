# Eye comfort ("Chống mỏi mắt") - owner requirement, 2026-11-27

Applies to the whole app UI (egui), in every theme. Goal: hours of subtitle work without eye strain, while the UI stays
as sharp, legible and good-looking as a normal theme. Comfort must come from colour/contrast/motion design, never from
blur, haze, low resolution, washed-out text or a global dimming overlay.

## Themes
`Light`, `Dark`, `System` (follows Windows) and a new **`Eye comfort`** mode. Eye comfort has its own warm palettes for a
light base and a dark base (setting: base = Light / Dark / follow Windows) and a **strength** slider 0-100 % (default 60 %),
like a blue-light filter but done properly inside the palette.

## Rules for ALL themes (Light and Dark are already "comfortable")
1. No pure extremes: Light bg is a soft off-white (not #FFFFFF), Dark bg is a dark grey (not #000000); body text never
   pure black/white. Body-text contrast 7:1 to 13:1 (WCAG AAA, but capped to avoid glare); secondary text >= 4.5:1;
   disabled >= 3:1. Checked automatically in CI for every token pair in every theme.
2. Large surfaces low-saturation (chroma capped); saturated colour only for small accents (selection border, markers,
   syntax tokens), each accent >= 3:1 against its background.
3. Selection/active-line colours are calm tints, not neon blocks; grid zebra stripes subtle (delta-L 2-4 %).
4. No flicker or flashing: caret blink off by default (option), no blinking warnings, transitions <= 150 ms and only on
   user action, honour Windows "Show animations" off; idle = zero repaints (already a budget).
5. Crisp text: correct DPI scaling, hinting/gamma-correct text rendering, minimum UI font 9 pt equivalent at 100 %,
   comfortable line height in grid/edit box; no fractional blurring of 1 px lines (snap to physical pixels).
6. Syntax highlighting, CPS warning colours, audio waveform/spectrum, keyframe/marker colours are tuned per theme with the
   same contrast rules; colour-blind safe (do not rely on red/green alone).

## Eye comfort mode (stronger)
1. Warm white point: palette converted in linear light with a chromatic adaptation (Bradford) from D65 towards ~3400 K at
   100 % strength (about 4500 K at default 60 %), so blue emission drops much more than in Light/Dark, while hue
   relationships and legibility stay intact. Accent blues become teal/amber equivalents of equal contrast.
2. Lower overall luminance spread: Light base ~ warm paper (e.g. L* 92-94 bg), Dark base ~ warm charcoal; body text
   contrast 7:1 to 10:1.
3. Measurable: at 100 % strength the UI's average blue-channel radiance (linear sRGB) is <= 50 % of the Light/Dark theme's,
   and contrast rules above still pass - asserted in tests for 0/30/60/100 %.
4. Video and subtitle preview are NOT tinted by default (typesetting needs true colours); option "Also warm the video
   preview" (off by default, with a note that colours are then not accurate). Colour pickers, colour swatches and hex
   values always show true colours.
5. Optional schedule: turn Eye comfort on/off by time of day (e.g. 19:00-07:00).

## Quality and speed
- Implemented as palette tokens computed once when the theme or strength changes (no per-frame shader overlay, no extra
  GPU pass). Theme switch or strength change < 16 ms, zero per-frame cost.
- Owner review: screenshots of grid + edit box + audio + video panels in Light / Dark / Eye comfort (light & dark base) at
  0/60/100 % on the owner's 2880x1800 200 % display before acceptance.

## Where it goes in the plan
Phase 2 ("Basic editor ... i18n + theme framework", Milestone A): theme framework ships with Light/Dark/System/Eye comfort
and the CI contrast tests. Every later phase (audio, video, visual tools) must add its colours as theme tokens and pass
the same tests. Pure palette logic (`crates/theme`) can be written ahead of Phase 2.
