# Aegisub 3.5.1 baseline (owner's PC)

Measured 2026-11-27 on the owner's PC (i7-1260P, 16 GB, Windows 11, 200% DPI) with the owner's tuned Aegisub 3.5.1, a real 1080p H.264 episode (35,609 frames) and its real .ass (31 styles). Read-only; no settings changed. Every phase report compares against this table.

| Metric | Aegisub 3.5.1 | hlouis Phase 0 | Budget (AGENTS.md) |
|---|---|---|---|
| Cold start to usable window | 6.35 s (≈5.5 s likely autoloaded automation/DependencyControl) | not measured yet | < 500 ms |
| Open video | 3.34 s (index cache likely present) | re-indexes every open (no cache yet) | first frame < 1 s with cache |
| Idle CPU (paused) | 0.31 % of one core | 0.63–3.1 % (continuous repaint) | < 0.5 % |
| Private memory, episode loaded | 505 MB (incl. audio) | 921 MB (no audio yet) | < 600 MB |
| 1080p playback, 60 s | not measured | 0 / 2,579 dropped | 0 dropped |
| Seek / frame step / keyframe latency | not measurable by script | not measurable by script | add instrumented benchmark (Phase 4) |

Gaps to close first: idle CPU and memory (perf-budget task), index cache (Phase 1/4).
