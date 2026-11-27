param(
    [string]$LogDir = "$env:APPDATA\Aegisub\log",
    [int]$MaxFiles = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $LogDir)) {
    Write-Warning "Aegisub log directory not found: $LogDir"
    exit 0
}

$files = Get-ChildItem $LogDir -Filter "*.json" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First $MaxFiles

if (-not $files) {
    Write-Host "No Aegisub log JSON files found in $LogDir"
    exit 0
}

function ConvertFrom-AegisubConcatJson([string]$Raw) {
    $chunks = ($Raw -split '\}\s*\{')
    $list = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt $chunks.Count; $i++) {
        $part = $chunks[$i].Trim()
        if (-not $part.StartsWith("{")) { $part = "{" + $part }
        if (-not $part.EndsWith("}")) { $part = $part + "}" }
        try {
            $list.Add(($part | ConvertFrom-Json))
        } catch {
            # Ignore truncated trailing objects if Aegisub is still running.
        }
    }
    return $list
}

Write-Host "=== Aegisub 3.5.1 Log Timing Summary (read-only) ==="
$startupSamples = New-Object System.Collections.Generic.List[double]

foreach ($f in $files) {
    $raw = Get-Content $f.FullName -Raw -ErrorAction SilentlyContinue
    if ([string]::IsNullOrWhiteSpace($raw)) { continue }
    $entries = ConvertFrom-AegisubConcatJson $raw
    if ($entries.Count -eq 0) { continue }

    $t0Ns = $null
    $displayNs = $null
    $leaveCtorNs = $null
    $videoLoadedNs = $null
    $libassInitNs = $null

    foreach ($e in $entries) {
        $ns = ([double]$e.sec * 1e9) + [double]$e.usec
        if ($null -eq $t0Ns) { $t0Ns = $ns }
        $secName = [string]$e.section
        $msg = [string]$e.message

        if ($secName -eq "frame_main/init" -and $msg -eq "Display main window" -and $null -eq $displayNs) {
            $displayNs = $ns
        }
        if ($secName -eq "frame_main/init" -and $msg -eq "Leaving FrameMain constructor" -and $null -eq $leaveCtorNs) {
            $leaveCtorNs = $ns
        }
        if ($secName -eq "subtitle/provider/libass" -and $msg -eq "Initialized" -and $null -eq $libassInitNs) {
            $libassInitNs = $ns
        }
        if ($secName -eq "video/out/gl" -and $msg -like "Video size:*" -and $null -eq $videoLoadedNs) {
            $videoLoadedNs = $ns
        }
    }

    $parts = @()
    if ($null -ne $displayNs -and $null -ne $t0Ns) {
        $ms = ($displayNs - $t0Ns) / 1e6
        $parts += ("display={0:F1}ms" -f $ms)
    }
    if ($null -ne $leaveCtorNs -and $null -ne $t0Ns) {
        $ms = ($leaveCtorNs - $t0Ns) / 1e6
        if ($ms -ge 0 -and $ms -lt 60000) {
            $startupSamples.Add($ms)
        }
        $parts += ("usable={0:F1}ms" -f $ms)
    }
    if ($null -ne $libassInitNs -and $null -ne $t0Ns) {
        $parts += ("libass={0:F1}ms" -f (($libassInitNs - $t0Ns) / 1e6))
    }
    if ($null -ne $videoLoadedNs -and $null -ne $t0Ns) {
        $parts += ("video_gl={0:F1}ms" -f (($videoLoadedNs - $t0Ns) / 1e6))
    }
    Write-Host ("{0,-28} entries={1,4}  {2}" -f $f.Name, $entries.Count, ($parts -join "  "))
}

if ($startupSamples.Count -gt 0) {
    $sorted = $startupSamples | Sort-Object
    $med = $sorted[[int]([Math]::Floor($sorted.Count / 2))]
    Write-Host ("Logged FrameMain startup delta median across {0} sessions: {1:F1} ms (excl. external process launch overhead)" -f $sorted.Count, $med)
}
