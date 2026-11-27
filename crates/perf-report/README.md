# `perf-report` — Criterion + `--bench-ui` Markdown Reporter

Generates `docs/perf-report.md` from Criterion JSON (`target/criterion/**/new/estimates.json`) and `hlouis-ui-demo --bench-ui` output.

## Local Usage

```bash
cargo run -p ui-shell --release --bin hlouis-ui-demo -- --bench-ui > bench-ui.log
cargo run -p perf-report --release -- --criterion target/criterion --bench-ui-log bench-ui.log --output docs/perf-report.md
```

## GitHub Actions PR Comment Snippet

```yaml
- name: Generate & attach performance report
  if: github.event_name == 'pull_request'
  env:
    GH_TOKEN: ${{ github.token }}
  run: |
    cargo run -p ui-shell --release --bin hlouis-ui-demo -- --bench-ui > bench-ui.log
    cargo run -p perf-report --release -- --criterion target/criterion --bench-ui-log bench-ui.log --output docs/perf-report.md
    gh pr comment "${{ github.event.pull_request.number }}" --body-file docs/perf-report.md
```
