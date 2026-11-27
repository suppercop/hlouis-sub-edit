#!/usr/bin/env bash
set -euo pipefail

# Generate initial seed corpora from plain-text fixtures/samples without committing binaries.
HLOUIS_WRITE_CORPUS=1 cargo check --manifest-path fuzz/Cargo.toml

SECONDS_PER_TARGET="${1:-60}"
TARGETS=(
  fuzz_ass_core
  fuzz_mkv_subs
  fuzz_keyframes
  fuzz_aegisub_config
  fuzz_settings
  fuzz_project
  fuzz_styles_catalog
  fuzz_karaskel
  fuzz_syntax_highlight
  fuzz_visual_tools
  fuzz_template_doctor
)

for target in "${TARGETS[@]}"; do
  echo "=== Fuzzing ${target} for ${SECONDS_PER_TARGET}s ==="
  cargo +nightly fuzz run --fuzz-dir fuzz --features libfuzzer "${target}" -- \
    -max_total_time="${SECONDS_PER_TARGET}" \
    -max_len=65536
done
