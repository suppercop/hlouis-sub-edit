<#
.SYNOPSIS
    Local fuzzing runner for Windows developers.
.DESCRIPTION
    Runs the deterministic stable-Rust smoke fuzzer (5,500+ mutated inputs across all 11 parsers)
    by default. Pass -Nightly to invoke cargo-fuzz on WSL2 / nightly Linux.
.EXAMPLE
    .\scripts\fuzz-local.ps1
    .\scripts\fuzz-local.ps1 -Target fuzz_ass_core -Seconds 60 -Nightly
#>
param(
    [string]$Target = "all",
    [int]$Seconds = 60,
    [switch]$Nightly
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $RepoRoot

try {
    if (-not $Nightly) {
        Write-Host "=== Running deterministic stable smoke fuzzer (all 11 parsers) ===" -ForegroundColor Cyan
        $env:HLOUIS_WRITE_CORPUS = "1"
        cargo test --manifest-path fuzz/Cargo.toml --test smoke_fuzz -- --nocapture
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Write-Host "All 11 parser smoke fuzzers passed with 0 panics." -ForegroundColor Green
        exit 0
    }

    $Targets = @(
        "fuzz_ass_core",
        "fuzz_mkv_subs",
        "fuzz_keyframes",
        "fuzz_aegisub_config",
        "fuzz_settings",
        "fuzz_project",
        "fuzz_styles_catalog",
        "fuzz_karaskel",
        "fuzz_syntax_highlight",
        "fuzz_visual_tools",
        "fuzz_template_doctor"
    )
    if ($Target -ne "all") {
        $Targets = @($Target)
    }

    $env:HLOUIS_WRITE_CORPUS = "1"
    cargo +nightly check --manifest-path fuzz/Cargo.toml
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    foreach ($t in $Targets) {
        Write-Host "=== Fuzzing $t for ${Seconds}s ===" -ForegroundColor Cyan
        cargo +nightly fuzz run --fuzz-dir fuzz --features libfuzzer $t -- "-max_total_time=$Seconds" "-max_len=65536"
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
}
finally {
    Pop-Location
}
