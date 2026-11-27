[CmdletBinding()]
param(
    [string]$CriterionDir = "target/criterion",
    [string]$CurrentBaseline = "pr",
    [string]$ReferenceBaseline = "main",
    [double]$MaxRegressionPct = 10.0,
    [string]$OutDir = "dist/bench-results"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not (Test-Path $CriterionDir)) {
    throw "Criterion directory not found at '$CriterionDir'"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Format-DurationNs {
    param([double]$Ns)
    if ($Ns -ge 1000000.0) {
        return ("{0:F3} ms" -f ($Ns / 1000000.0))
    }
    elseif ($Ns -ge 1000.0) {
        return ("{0:F2} us" -f ($Ns / 1000.0))
    }
    return ("{0:F1} ns" -f $Ns)
}

$benchDirs = Get-ChildItem -Path $CriterionDir -Recurse -Filter "estimates.json" |
    Where-Object { $_.Directory.Name -eq $CurrentBaseline } |
    ForEach-Object { $_.Directory.Parent } |
    Sort-Object FullName -Unique

if (-not $benchDirs -or $benchDirs.Count -eq 0) {
    throw "No Criterion '$CurrentBaseline' estimates.json files found under '$CriterionDir'"
}

$rows = @()
$regressions = @()
$compareMode = ($CurrentBaseline -ne $ReferenceBaseline)

foreach ($benchDir in $benchDirs) {
    $relName = $benchDir.FullName.Substring((Resolve-Path $CriterionDir).Path.Length).TrimStart('\', '/').Replace('\', '/')
    $curJsonPath = Join-Path $benchDir.FullName "$CurrentBaseline\estimates.json"
    $curData = Get-Content -Raw $curJsonPath | ConvertFrom-Json
    $curMedianNs = [double]$curData.median.point_estimate
    $curMeanNs = [double]$curData.mean.point_estimate

    $refJsonPath = Join-Path $benchDir.FullName "$ReferenceBaseline\estimates.json"
    $baseMedianNs = $null
    $deltaPct = $null
    $status = "OK"

    if ($compareMode -and (Test-Path $refJsonPath)) {
        $refData = Get-Content -Raw $refJsonPath | ConvertFrom-Json
        $baseMedianNs = [double]$refData.median.point_estimate
        if ($baseMedianNs -gt 0.0) {
            $deltaPct = (($curMedianNs - $baseMedianNs) / $baseMedianNs) * 100.0
            if ($deltaPct -gt $MaxRegressionPct) {
                $status = "REGRESSION"
                $regressions += "$relName (+$("{0:F2}" -f $deltaPct)%)"
            }
            elseif ($deltaPct -lt -5.0) {
                $status = "FASTER"
            }
        }
    }
    elseif ($compareMode) {
        $status = "NO_BASELINE"
    }

    $rows += [PSCustomObject]@{
        Benchmark       = $relName
        BaselineMedian  = if ($null -ne $baseMedianNs) { Format-DurationNs $baseMedianNs } else { "N/A" }
        CurrentMedian   = Format-DurationNs $curMedianNs
        CurrentMean     = Format-DurationNs $curMeanNs
        Delta           = if ($null -ne $deltaPct) { ("{0:+0.00;-0.00;0.00}%" -f $deltaPct) } else { "N/A" }
        Status          = $status
        BaselineNs      = $baseMedianNs
        CurrentMedianNs = $curMedianNs
        CurrentMeanNs   = $curMeanNs
        DeltaPct        = $deltaPct
    }
}

$mdLines = @(
    "### Criterion Benchmark Results ($CurrentBaseline vs $ReferenceBaseline, threshold +$MaxRegressionPct%)"
    ""
    "| Benchmark | Baseline ($ReferenceBaseline) | Current ($CurrentBaseline) | Mean | Delta | Status |"
    "|---|---|---|---|---|---|"
)
foreach ($r in $rows) {
    $mdLines += "| ``$($r.Benchmark)`` | $($r.BaselineMedian) | $($r.CurrentMedian) | $($r.CurrentMean) | $($r.Delta) | $($r.Status) |"
}
$md = $mdLines -join "`n"

Write-Host $md
Set-Content -Path (Join-Path $OutDir "summary.md") -Value $md -Encoding UTF8
$rows | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $OutDir "results.json") -Encoding UTF8

if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_STEP_SUMMARY)) {
    Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value "`n$md`n" -Encoding UTF8
}

if ($regressions.Count -gt 0) {
    throw "Benchmark regression(s) exceeding +$MaxRegressionPct% detected: $($regressions -join ', ')"
}
