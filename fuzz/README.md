# HLouis Sub Edit — Fuzzing Suite

Standalone `cargo-fuzz` / libFuzzer workspace excluded from the main Cargo workspace (`exclude = ["fuzz"]`) so stable Windows/Linux builds never require nightly or `libfuzzer-sys`.

## Targets

| Target | Crate(s) Exercised |
|---|---|
| `fuzz_ass_core` | `ass-core` (ASS parser, charset detector, SRT parser, override tag blocks, round-trip) |
| `fuzz_mkv_subs` | `mkv-subs` (EBML/Matroska track listing + single-pass subtitle extraction with lacing & zlib) |
| `fuzz_keyframes` | `keyframes` (keyframe formats, timecodes v1/v2, writers) |
| `fuzz_aegisub_config` | `aegisub-config` (`config.json` and `hotkey.json` importers) |
| `fuzz_settings` | `settings` (`AppSettings::load_json`, `migrate`, `validate_and_clamp`, `save_json`) |
| `fuzz_project` | `project` (`[Aegisub Project Garbage]` read/write) |
| `fuzz_styles_catalog` | `styles-catalog` (`.sty` parse + serialize round-trip) |
| `fuzz_karaskel` | `karaskel` (template classification, head collection, karaoke preprocessing) |
| `fuzz_syntax_highlight` | `syntax-highlight` (override tokenizer, incremental `re_tokenize_edit`, grid formatter) |
| `fuzz_visual_tools` | `visual-tools` (vector clip parser, `b`/`s` conversions, `\move`, tag upsert) |
| `fuzz_template_doctor` | `template-doctor` (ASS/template/Lua 5.1 linter + idempotent `fix_all_safe`) |

## Quick Smoke Test (Stable Rust — Linux or Windows)

```bash
cargo test --manifest-path fuzz/Cargo.toml --test smoke_fuzz
```

## Full libFuzzer Run (Nightly Linux CI)

Install `cargo-fuzz` once:

```bash
cargo install cargo-fuzz
```

Run every target for 60 seconds with auto-generated seed corpora (no binary seeds committed):

```bash
./fuzz/ci_fuzz.sh
```

Or run a single target:

```bash
HLOUIS_WRITE_CORPUS=1 cargo check --manifest-path fuzz/Cargo.toml
cargo +nightly fuzz run --fuzz-dir fuzz --features libfuzzer fuzz_ass_core -- -max_total_time=60
```
