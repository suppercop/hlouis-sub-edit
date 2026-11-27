param(
    [string]$AegisubExe = "C:\Program Files\Aegisub\aegisub.exe",
    [string]$VideoPath = "",
    [string]$AssPath = "",
    [switch]$RunInteractive,
    [switch]$MeasureSideBySide
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$benchDir = Join-Path $env:TEMP "aegisub-bench"
New-Item -ItemType Directory -Force -Path $benchDir | Out-Null

if ([string]::IsNullOrWhiteSpace($AssPath)) {
    $AssPath = Join-Path $benchDir "bench_20k.ass"
    $sb = New-Object System.Text.StringBuilder (2600000)
    [void]$sb.Append([char]0xFEFF)
    [void]$sb.AppendLine("[Script Info]")
    [void]$sb.AppendLine("Title: Bench 20k")
    [void]$sb.AppendLine("ScriptType: v4.00+")
    [void]$sb.AppendLine("PlayResX: 1920")
    [void]$sb.AppendLine("PlayResY: 1080")
    [void]$sb.AppendLine("")
    [void]$sb.AppendLine("[V4+ Styles]")
    [void]$sb.AppendLine("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding")
    [void]$sb.AppendLine("Style: Default,Segoe UI,54,&H00FFFFFF,&H0000FFFF,&H00181820,&H90000000,-1,0,0,0,100,100,0,0,1,3,1.5,2,40,40,36,1")
    [void]$sb.AppendLine("")
    [void]$sb.AppendLine("[Events]")
    [void]$sb.AppendLine("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text")

    $templates = @(
        '{\an8\fs44\c&H00FFFF&}Xin chào Tiếng Việt\N{\k30}Ka{\kf45}raoke — 日本語テスト',
        '{\pos(960,980)\fad(150,150)}日本語の字幕テスト：漢字・ひらがな・カタカナ',
        '{\an5\frz-12\t(0,400,\fscx115\fscy115)}Styled sign\hwith\hnon-breaking spaces',
        'Plain dialogue line with punctuation and normal text.'
    )

    for ($i = 0; $i -lt 20000; $i++) {
        $startCs = if ($i -eq 0) { 0 } else { $i * 150 }
        $endCs = if ($i -eq 0) { 1000 } else { $startCs + 140 }
        $sSec = [int][Math]::Floor($startCs / 100); $sCc = $startCs % 100
        $eSec = [int][Math]::Floor($endCs / 100); $eCc = $endCs % 100
        $sStr = ("{0}:{1:D2}:{2:D2}.{3:D2}" -f [int][Math]::Floor($sSec / 3600), [int][Math]::Floor(($sSec % 3600) / 60), ($sSec % 60), $sCc)
        $eStr = ("{0}:{1:D2}:{2:D2}.{3:D2}" -f [int][Math]::Floor($eSec / 3600), [int][Math]::Floor(($eSec % 3600) / 60), ($eSec % 60), $eCc)
        [void]$sb.AppendLine(("Dialogue: 0,{0},{1},Default,Speaker,0,0,0,,{2} [#{3}]" -f $sStr, $eStr, $templates[$i % 4], $i))
    }
    [System.IO.File]::WriteAllText($AssPath, $sb.ToString(), [System.Text.Encoding]::UTF8)
    Write-Host "Generated 20,000-line ASS fixture at $AssPath"
}

& (Join-Path $PSScriptRoot "parse-aegisub-logs.ps1")

if (-not $RunInteractive) {
    Write-Host ""
    Write-Host "Safe read-only mode complete (no edits made to %APPDATA%\Aegisub)."
    Write-Host "Pass -RunInteractive to launch Aegisub 3.5.1 with UI Automation startup/keystroke timing."
    exit 0
}

if (-not (Test-Path $AegisubExe)) {
    throw "Aegisub executable not found at $AegisubExe"
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$argList = @($AssPath)
if (-not [string]::IsNullOrWhiteSpace($VideoPath) -and (Test-Path $VideoPath)) {
    $argList += $VideoPath
}

Write-Host "Launching Aegisub 3.5.1 ($AegisubExe)..."
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$proc = Start-Process -FilePath $AegisubExe -ArgumentList $argList -PassThru
$null = $proc.WaitForInputIdle(30000)

while ($proc.MainWindowHandle -eq [IntPtr]::Zero -and -not $proc.HasExited -and $sw.ElapsedMilliseconds -lt 30000) {
    Start-Sleep -Milliseconds 10
    $proc.Refresh()
}

$root = $null
if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($proc.MainWindowHandle)
}
$coldStartMs = $sw.Elapsed.TotalMilliseconds
Start-Sleep -Milliseconds 1500
$proc.Refresh()
$idleMemMb = $proc.PrivateMemorySize64 / (1024.0 * 1024.0)

Write-Host ("Aegisub startup + 20k open: {0:F1} ms | Private RAM: {1:F1} MiB" -f $coldStartMs, $idleMemMb)

# Keystroke -> window pixel-change timing inside focused Edit Box using UIA + GDI crop
$editCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Edit
)
$keyLatencies = New-Object System.Collections.Generic.List[double]
if ($null -ne $root) {
    $edits = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $editCond)
    if ($edits.Count -gt 0) {
        $targetEdit = $edits[$edits.Count - 1]
        $rect = $targetEdit.Current.BoundingRectangle
        if ($rect.Width -gt 20 -and $rect.Height -gt 10) {
            try { $targetEdit.SetFocus() } catch {}
            Start-Sleep -Milliseconds 150
            $bmpBefore = New-Object System.Drawing.Bitmap ([int]$rect.Width), ([int]$rect.Height)
            $bmpAfter = New-Object System.Drawing.Bitmap ([int]$rect.Width), ([int]$rect.Height)
            $gBefore = [System.Drawing.Graphics]::FromImage($bmpBefore)
            $gAfter = [System.Drawing.Graphics]::FromImage($bmpAfter)
            for ($k = 0; $k -lt 20; $k++) {
                $gBefore.CopyFromScreen([int]$rect.X, [int]$rect.Y, 0, 0, $bmpBefore.Size)
                $hashBefore = $bmpBefore.GetPixel([int]($rect.Width / 2), [int]($rect.Height / 2)).ToArgb()
                $kSw = [System.Diagnostics.Stopwatch]::StartNew()
                [System.Windows.Forms.SendKeys]::SendWait("a")
                while ($kSw.ElapsedMilliseconds -lt 250) {
                    $gAfter.CopyFromScreen([int]$rect.X, [int]$rect.Y, 0, 0, $bmpAfter.Size)
                    $changed = $false
                    for ($sx = 8; $sx -lt ([int]$rect.Width - 8); $sx += 12) {
                        if ($bmpAfter.GetPixel($sx, [int]($rect.Height / 2)).ToArgb() -ne $bmpBefore.GetPixel($sx, [int]($rect.Height / 2)).ToArgb()) {
                            $changed = $true; break
                        }
                    }
                    if ($changed) { break }
                    Start-Sleep -Milliseconds 1
                }
                $keyLatencies.Add($kSw.Elapsed.TotalMilliseconds)
            }
            $gBefore.Dispose(); $gAfter.Dispose(); $bmpBefore.Dispose(); $bmpAfter.Dispose()
        }
    }
}

if ($keyLatencies.Count -gt 0) {
    $sortedKeys = $keyLatencies | Sort-Object
    $p95Idx = [Math]::Min($sortedKeys.Count - 1, [int][Math]::Ceiling($sortedKeys.Count * 0.95) - 1)
    Write-Host ("Aegisub UIA EditBox keystroke->pixel p95: {0:F1} ms (n={1})" -f $sortedKeys[$p95Idx], $sortedKeys.Count)
}

Write-Host "Check %TEMP%\aegisub_lua_bench.json after invoking Automation -> HLouis Benchmark (20k Ops) in Aegisub."
if (-not $proc.HasExited) {
    $null = $proc.CloseMainWindow()
}

if ($MeasureSideBySide) {
    $repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
    $hlExe = Join-Path $repoRoot "target\release\hlouis-sub-edit.exe"
    if (Test-Path $hlExe) {
        Write-Host ""
        Write-Host "=== Side-by-side hlouis-sub-edit --bench-ui ==="
        if (-not [string]::IsNullOrWhiteSpace($VideoPath) -and (Test-Path $VideoPath)) {
            & $hlExe --bench-ui --video $VideoPath
        } else {
            & $hlExe --bench-ui
        }
    }
}
