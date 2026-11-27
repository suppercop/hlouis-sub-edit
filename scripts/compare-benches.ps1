[CmdletBinding()]
param(
    [string]$CriterionDir = "target/criterion",
    [string]$CurrentBaseline = "pr",
    [string]$ReferenceBaseline = "main",
    [double]$MaxRegressionPct = 10.0,
    [string]$CeilingsPath = "",
    [switch]$FailOnCeiling,
    [string]$RecordMedianJson = "",
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

$ceilingsMap = @{}
if (-not [string]::IsNullOrWhiteSpace($CeilingsPath) -and (Test-Path $CeilingsPath)) {
    $cJson = Get-Content -Raw $CeilingsPath | ConvertFrom-Json
    if ($null -ne $cJson.benchmarks) {
        foreach ($prop in $cJson.benchmarks.PSObject.Properties) {
            $ceilingsMap[$prop.Name] = $prop.Value
        }
    }
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
$ceilingBreaches = @()
$mediansExport = [ordered]@{}
$compareMode = ($CurrentBaseline -ne $ReferenceBaseline)

foreach ($benchDir in $benchDirs) {
    $relName = $benchDir.FullName.Substring((Resolve-Path $CriterionDir).Path.Length).TrimStart('\', '/').Replace('\', '/')
    $curJsonPath = Join-Path $benchDir.FullName "$CurrentBaseline\estimates.json"
    $curData = Get-Content -Raw $curJsonPath | ConvertFrom-Json
    $curMedianNs = [double]$curData.median.point_estimate
    $curMeanNs = [double]$curData.mean.point_estimate
    $mediansExport[$relName] = $curMedianNs

    $refJsonPath = Join-Path $benchDir.FullName "$ReferenceBaseline\estimates.json"
    $baseMedianNs = $null
    $deltaPct = $null
    $statuses = @()

    if ($compareMode -and (Test-Path $refJsonPath)) {
        $refData = Get-Content -Raw $refJsonPath | ConvertFrom-Json
        $baseMedianNs = [double]$refData.median.point_estimate
        if ($baseMedianNs -gt 0.0) {
            $deltaPct = (($curMedianNs - $baseMedianNs) / $baseMedianNs) * 100.0
            if ($deltaPct -gt $MaxRegressionPct) {
                $statuses += "REGRESSION"
                $regressions += "$relName (+$("{0:F2}" -f $deltaPct)%)"
            }
            elseif ($deltaPct -lt -5.0) {
                $statuses += "FASTER"
            }
        }
    }
    elseif ($compareMode) {
        $statuses += "NO_BASELINE"
    }

    $ciMaxNs = $null
    if ($ceilingsMap.ContainsKey($relName)) {
        $entry = $ceilingsMap[$relName]
        if ($null -ne $entry.ci_max_ns) {
            $ciMaxNs = [double]$entry.ci_max_ns
            if ($curMedianNs -gt $ciMaxNs) {
                $statuses += "OVER_CEILING"
                if ($FailOnCeiling) {
                    $ceilingBreaches += "$relName ($(Format-DurationNs $curMedianNs) > $(Format-DurationNs $ciMaxNs))"
                }
            }
        }
    }

    $statusStr = if ($statuses.Count -eq 0) { "OK" } else { $statuses -join "+" }

    $rows += [PSCustomObject]@{
        Benchmark       = $relName
        BaselineMedian  = if ($null -ne $baseMedianNs) { Format-DurationNs $baseMedianNs } else { "N/A" }
        CurrentMedian   = Format-DurationNs $curMedianNs
        CurrentMean     = Format-DurationNs $curMeanNs
        Delta           = if ($null -ne $deltaPct) { ("{0:+0.00;-0.00;0.00}%" -f $deltaPct) } else { "N/A" }
        CiCeiling       = if ($null -ne $ciMaxNs) { Format-DurationNs $ciMaxNs } else { "uncalibrated" }
        Status          = $statusStr
        BaselineNs      = $baseMedianNs
        CurrentMedianNs = $curMedianNs
        CurrentMeanNs   = $curMeanNs
        DeltaPct        = $deltaPct
        CiMaxNs         = $ciMaxNs
    }
}

$mdLines = @(
    "### Criterion Benchmark Results ($CurrentBaseline vs $ReferenceBaseline, regression +$MaxRegressionPct%, ceiling enforcement=$FailOnCeiling)"
    ""
    "| Benchmark | Baseline ($ReferenceBaseline) | Current ($CurrentBaseline) | Mean | Delta | CI Ceiling | Status |"
    "|---|---|---|---|---|---|---|"
)
foreach ($r in $rows) {
    $mdLines += "| ``$($r.Benchmark)`` | $($r.BaselineMedian) | $($r.CurrentMedian) | $($r.CurrentMean) | $($r.Delta) | $($r.CiCeiling) | $($r.Status) |"
}
$md = $mdLines -join "`n"

Write-Host $md
Set-Content -Path (Join-Path $OutDir "summary.md") -Value $md -Encoding UTF8
$rows | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $OutDir "results.json") -Encoding UTF8

if (-not [string]::IsNullOrWhiteSpace($RecordMedianJson)) {
    $parent = Split-Path -Parent $RecordMedianJson
    if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
    $mediansExport | ConvertTo-Json -Depth 4 | Set-Content -Path $RecordMedianJson -Encoding UTF8
}

if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_STEP_SUMMARY)) {
    Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value "`n$md`n" -Encoding UTF8
}

$errors = @()
if ($regressions.Count -gt 0) {
    $errors += "Benchmark regression(s) exceeding +$MaxRegressionPct%: $($regressions -join ', ')"
}
if ($FailOnCeiling -and $ceilingBreaches.Count -gt 0) {
    $errors += "Benchmark ceiling(s) exceeded: $($ceilingBreaches -join ', ')"
}
if ($errors.Count -gt 0) {
    throw ($errors -join " | ")
}
