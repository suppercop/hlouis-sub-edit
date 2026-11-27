# `golden-tools` — Aegisub 3.5.1 Golden Output Harness

Compares `ass-tools` (`Shift Times`, `Timing Post-Processor`, `Resample Resolution`) against 10 expected `.ass` fixtures in `tests/fixtures/expected/`.

## Running the Harness

```bash
cargo test -p golden-tools
```

## Regenerating Expected Outputs with Real Aegisub 3.5.1 (Windows)

For each fixture in `tests/fixtures/input/<case>.ass`, open it in **Aegisub 3.5.1** on Windows, run the exact operation below, save to `tests/fixtures/expected/<case>.ass`, and re-run `cargo test -p golden-tools`:

1. `01_shift_forward_2s.ass`: **Timing → Shift Times** → `0:00:02.00`, Forward, All rows, Start and End.
2. `02_shift_backward_clamp_zero.ass`: **Timing → Shift Times** → `0:00:05.00`, Backward, All rows, Start and End (clamps at `0:00:00.00`).
3. `03_shift_start_only_selected.ass`: Select row 2 (0-based index 1) → **Timing → Shift Times** → `0:00:00.50`, Forward, Selected rows, Start times only.
4. `04_shift_end_only_onward.ass`: Select row 2 → **Timing → Shift Times** → `0:00:01.00`, Forward, Selection onward, End times only.
5. `05_tpp_lead_in_out.ass`: **Timing → Timing Post-Processor** → Lead-in `100 ms`, Lead-out `200 ms`, skip comments.
6. `06_tpp_make_adjacent_midpoint.ass`: **Timing → Timing Post-Processor** → Make adjacent continuous, threshold `500 ms`, bias `0.5` (50%), prevent overlap.
7. `07_tpp_keyframe_snap.ass`: Load dummy keyframes at `2.00 s`, `5.00 s`, `9.00 s` → **Timing → Timing Post-Processor** → Keyframe snapping, all 4 thresholds `150 ms`.
8. `08_resample_720p_to_1080p.ass`: **Subtitle → Resample Resolution** → Destination `1920 × 1080`, Add Borders (preserve AR).
9. `09_resample_1080p_to_720p.ass`: **Subtitle → Resample Resolution** → Destination `1280 × 720`, Add Borders.
10. `10_resample_stretch_pos_clip.ass`: **Subtitle → Resample Resolution** → Destination `1920 × 1080`, Stretch (anamorphic).

Any diff reported by `cargo test -p golden-tools` after regenerating with Aegisub 3.5.1 pinpoints an exact parity discrepancy to fix in `crates/ass-tools`.
