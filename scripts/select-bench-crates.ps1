param(
    [string]$BaseRef = "main",
    [switch]$All
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($All) {
    Write-Output "--workspace"
    exit 0
}

# Build map of crates/<dir> -> Cargo package name (only for crates defining [[bench]]).
$crateMap = @{}
foreach ($toml in Get-ChildItem -Path "crates" -Filter "Cargo.toml" -Depth 1) {
    $raw = Get-Content -Raw $toml.FullName
    if ($raw -notmatch '\[\[bench\]\]') {
        continue
    }
    if ($raw -match '(?m)^\s*name\s*=\s*"([^"]+)"') {
        $pkgName = $Matches[1]
        $dirRel = "crates/$($toml.Directory.Name)/"
        $crateMap[$dirRel] = $pkgName
    }
}

$selected = New-Object System.Collections.Generic.HashSet[string]
# Always benchmark ass-core to keep a baseline sanity signal on every PR.
if ($crateMap.Values -contains "ass-core") {
    [void]$selected.Add("ass-core")
}

$changedFiles = @()
$diffTarget = "origin/$BaseRef...HEAD"
$null = git rev-parse --verify "origin/$BaseRef" 2>$null
if ($LASTEXITCODE -eq 0) {
    $changedFiles = @(git diff --name-only $diffTarget)
} else {
    $changedFiles = @(git diff --name-only "HEAD~1..HEAD" 2>$null)
}

foreach ($file in $changedFiles) {
    $norm = $file.Replace('\', '/')
    foreach ($prefix in $crateMap.Keys) {
        if ($norm.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            $rel = $norm.Substring($prefix.Length)
            if ($rel -match '^(src/|benches/|Cargo\.toml$|build\.rs$)') {
                [void]$selected.Add($crateMap[$prefix])
            }
        }
    }
}

if ($selected.Count -eq 0) {
    Write-Output "--workspace"
    exit 0
}

$argsOut = @()
foreach ($pkg in ($selected | Sort-Object)) {
    $argsOut += "-p"
    $argsOut += $pkg
}
Write-Output ($argsOut -join " ")
