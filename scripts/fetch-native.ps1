[CmdletBinding()]
param(
    [string]$OutputDir = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot ".native"
}

$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)
$DownloadsDir = Join-Path $OutputDir "downloads"
$WorkDir = Join-Path $OutputDir "work"
$VcpkgInstalledDir = Join-Path $OutputDir "vcpkg_installed"
$IncludeOut = Join-Path $OutputDir "include"
$LibOut = Join-Path $OutputDir "lib"
$BinOut = Join-Path $OutputDir "bin"
$LicensesOut = Join-Path $OutputDir "licenses"

foreach ($dir in @($OutputDir, $DownloadsDir, $WorkDir, $IncludeOut, $LibOut, $BinOut, $LicensesOut)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

function Initialize-MsvcEnv {
    if (Get-Command cl.exe -ErrorAction SilentlyContinue) {
        Write-Host "[native] MSVC cl.exe already on PATH"
        return
    }

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        throw "vswhere.exe not found at '$vswhere'. Install Visual Studio Build Tools with C++ x64 workload."
    }

    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ([string]::IsNullOrWhiteSpace($vsPath)) {
        throw "Visual Studio C++ x64 build tools not found via vswhere."
    }

    $devShell = Join-Path $vsPath "Common7\Tools\Launch-VsDevShell.ps1"
    if (-not (Test-Path $devShell)) {
        throw "Launch-VsDevShell.ps1 not found at '$devShell'."
    }

    Write-Host "[native] Loading MSVC x64 environment from $vsPath"
    & $devShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        throw "cl.exe still not found after Launch-VsDevShell.ps1"
    }
}

function Invoke-PinnedDownload {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$DestPath,
        [Parameter(Mandatory = $true)][string]$ExpectedSha256
    )

    $Expected = $ExpectedSha256.ToUpperInvariant()
    if ((-not $Force) -and (Test-Path $DestPath)) {
        $existingHash = (Get-FileHash -Path $DestPath -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($existingHash -eq $Expected) {
            Write-Host "[native] Verified cached $(Split-Path $DestPath -Leaf) ($Expected)"
            return
        }
        Write-Warning "[native] Hash mismatch on cached $DestPath, re-downloading..."
    }

    Write-Host "[native] Downloading $Url ..."
    & curl.exe -fL --retry 3 --retry-delay 2 -o $DestPath $Url
    if ($LASTEXITCODE -ne 0) {
        if (($Url -match '^https://github\.com/([^/]+/[^/]+)/releases/download/([^/]+)/([^/?]+)$') -and (Get-Command gh.exe -ErrorAction SilentlyContinue)) {
            $ghRepo = $Matches[1]
            $ghTag = $Matches[2]
            $ghAsset = $Matches[3]
            Write-Host "[native] Direct download failed; trying gh release download ($ghRepo $ghTag/$ghAsset) ..."
            & gh.exe release download $ghTag --repo $ghRepo --pattern $ghAsset --output $DestPath --clobber
            if ($LASTEXITCODE -ne 0) {
                throw "Failed downloading $Url via curl and gh release download (exit $LASTEXITCODE)"
            }
        }
        else {
            throw "curl.exe failed downloading $Url (exit $LASTEXITCODE)"
        }
    }

    $actualHash = (Get-FileHash -Path $DestPath -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($actualHash -ne $Expected) {
        Remove-Item -Force -ErrorAction SilentlyContinue $DestPath
        throw "SHA-256 verification failed for $DestPath! Expected $Expected, got $actualHash"
    }
    Write-Host "[native] Verified SHA-256 for $(Split-Path $DestPath -Leaf)"
}

function Resolve-Vcpkg {
    foreach ($varName in @("VCPKG_ROOT", "VCPKG_INSTALLATION_ROOT")) {
        $candidate = [Environment]::GetEnvironmentVariable($varName)
        if (-not [string]::IsNullOrWhiteSpace($candidate)) {
            $exe = Join-Path $candidate "vcpkg.exe"
            if (Test-Path $exe) {
                Write-Host "[native] Using existing vcpkg at $candidate"
                return $candidate
            }
        }
    }

    $localVcpkg = Join-Path $OutputDir "vcpkg-tool"
    $localExe = Join-Path $localVcpkg "vcpkg.exe"
    if (-not (Test-Path $localExe)) {
        if (-not (Test-Path (Join-Path $localVcpkg ".git"))) {
            Write-Host "[native] Cloning microsoft/vcpkg into $localVcpkg ..."
            for ($attempt = 1; $attempt -le 3; $attempt++) {
                if (Test-Path $localVcpkg) {
                    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $localVcpkg
                }
                & git clone --filter=blob:none --depth 1 https://github.com/microsoft/vcpkg.git $localVcpkg | Out-Host
                if ($LASTEXITCODE -eq 0) {
                    break
                }
                if ($attempt -eq 3) {
                    throw "Failed to clone vcpkg after 3 attempts"
                }
                Write-Warning "[native] vcpkg clone attempt $attempt failed, retrying..."
                Start-Sleep -Seconds 3
            }
        }
        Write-Host "[native] Bootstrapping vcpkg ..."
        & (Join-Path $localVcpkg "bootstrap-vcpkg.bat") -disableMetrics | Out-Host
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to bootstrap vcpkg"
        }
    }
    return $localVcpkg
}

Initialize-MsvcEnv

# 1. Install libass + zlib via vcpkg manifest (x64-windows dynamic DLLs)
$vcpkgRoot = Resolve-Vcpkg
$vcpkgExe = Join-Path $vcpkgRoot "vcpkg.exe"

Push-Location $RepoRoot
try {
    Write-Host "[native] Installing libass and zlib (x64-windows) via vcpkg manifest ..."
    & $vcpkgExe install --triplet x64-windows --x-install-root="$VcpkgInstalledDir" --vcpkg-root="$vcpkgRoot" --no-print-usage
    if ($LASTEXITCODE -ne 0) {
        throw "vcpkg install failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$vcpkgTripletDir = Join-Path $VcpkgInstalledDir "x64-windows"

# 2. Download pinned LGPL FFmpeg shared build (n9.0.2-982-g836df4e858, LGPL-3.0) from native-deps-v1 release
$ffmpegUrl = "https://github.com/suppercop/hlouis-sub-edit/releases/download/native-deps-v1/ffmpeg-n9.0.2-982-g836df4e858-win64-lgpl-shared-9.0.zip"
$ffmpegSha256 = "F7BA27E7986A2D0835881E742A682DEC17896F6DC2CDCF9E596F043D2763D894"
$ffmpegArchive = Join-Path $DownloadsDir "ffmpeg-n9.0.2-982-g836df4e858-win64-lgpl-shared-9.0.zip"
Invoke-PinnedDownload -Url $ffmpegUrl -DestPath $ffmpegArchive -ExpectedSha256 $ffmpegSha256

$ffmpegRoot = Join-Path $WorkDir "ffmpeg-n9.0.2-982-g836df4e858-win64-lgpl-shared-9.0"
if ($Force -or (-not (Test-Path (Join-Path $ffmpegRoot "lib\avcodec.lib")))) {
    Write-Host "[native] Extracting FFmpeg LGPL shared build ..."
    if (Test-Path $ffmpegRoot) {
        Remove-Item -Recurse -Force $ffmpegRoot
    }
    & tar.exe -xf $ffmpegArchive -C $WorkDir
    if ($LASTEXITCODE -ne 0) {
        throw "tar.exe failed extracting $ffmpegArchive"
    }
}

# 3. Download pinned FFMS2 source (commit 3af2ef2ae47bc30b64597c9e419e5b19c4bda7d8, MIT)
$ffmsUrl = "https://github.com/FFMS/ffms2/archive/3af2ef2ae47bc30b64597c9e419e5b19c4bda7d8.tar.gz"
$ffmsSha256 = "FB7AE3D336D1F49E2A7FADE721833778FC312D31FB9F3C811EAF81CC4A170517"
$ffmsArchive = Join-Path $DownloadsDir "ffms2-3af2ef2a.tar.gz"
Invoke-PinnedDownload -Url $ffmsUrl -DestPath $ffmsArchive -ExpectedSha256 $ffmsSha256

$ffmsRoot = Join-Path $WorkDir "ffms2-3af2ef2ae47bc30b64597c9e419e5b19c4bda7d8"
if ($Force -or (-not (Test-Path (Join-Path $ffmsRoot "include\ffms.h")))) {
    Write-Host "[native] Extracting FFMS2 source ..."
    if (Test-Path $ffmsRoot) {
        Remove-Item -Recurse -Force $ffmsRoot
    }
    & tar.exe -xf $ffmsArchive -C $WorkDir
    if ($LASTEXITCODE -ne 0) {
        throw "tar.exe failed extracting $ffmsArchive"
    }
}

# 4. Compile ffms2.dll + ffms2.lib (MIT) dynamically linked against LGPL FFmpeg + zlib
$ffmsOutDll = Join-Path $BinOut "ffms2.dll"
$ffmsOutLib = Join-Path $LibOut "ffms2.lib"
$ffmsStampPath = Join-Path $OutputDir "ffms2-ffmpeg.stamp"
$desiredStamp = "$ffmpegSha256|3af2ef2ae47bc30b64597c9e419e5b19c4bda7d8"
$currentStamp = if (Test-Path $ffmsStampPath) { (Get-Content -Raw $ffmsStampPath).Trim() } else { "" }
if ($Force -or (-not (Test-Path $ffmsOutDll)) -or (-not (Test-Path $ffmsOutLib)) -or ($currentStamp -ne $desiredStamp)) {
    Write-Host "[native] Compiling ffms2.dll with MSVC ..."
    $ffmsBuildDir = Join-Path $WorkDir "ffms2-obj"
    New-Item -ItemType Directory -Force -Path $ffmsBuildDir | Out-Null

    $coreSources = @(
        "audiosource.cpp",
        "ffms.cpp",
        "filehandle.cpp",
        "indexing.cpp",
        "track.cpp",
        "utils.cpp",
        "videosource.cpp",
        "videoutils.cpp",
        "zipfile.cpp"
    ) | ForEach-Object { Join-Path $ffmsRoot "src\core\$_" }

    $compileRspLines = @(
        "/nologo"
        "/c"
        "/O2"
        "/EHsc"
        "/MD"
        "/std:c++17"
        "/DFFMS_EXPORTS"
        "/DFFMS2_EXPORTS"
        "/D_CRT_SECURE_NO_WARNINGS"
        "/D__STDC_CONSTANT_MACROS"
        "/DNDEBUG"
        "/D_WINDOWS"
        "/D_USRDLL"
        "/I`"$(Join-Path $ffmsRoot 'include')`""
        "/I`"$(Join-Path $ffmpegRoot 'include')`""
        "/I`"$(Join-Path $vcpkgTripletDir 'include')`""
        "/Fo`"$ffmsBuildDir\\`""
    )
    foreach ($src in $coreSources) {
        $compileRspLines += "`"$src`""
    }

    $compileRspPath = Join-Path $ffmsBuildDir "compile.rsp"
    Set-Content -Path $compileRspPath -Value ($compileRspLines -join "`r`n") -Encoding ASCII

    & cl.exe "@$compileRspPath"
    if ($LASTEXITCODE -ne 0) {
        throw "cl.exe failed compiling ffms2 objects (exit $LASTEXITCODE)"
    }

    $linkRspLines = @(
        "/nologo"
        "/DLL"
        "/OUT:`"$ffmsOutDll`""
        "/IMPLIB:`"$ffmsOutLib`""
        "/LIBPATH:`"$(Join-Path $ffmpegRoot 'lib')`""
        "/LIBPATH:`"$(Join-Path $vcpkgTripletDir 'lib')`""
        "avformat.lib"
        "avcodec.lib"
        "avutil.lib"
        "swscale.lib"
        "swresample.lib"
        "libz.lib"
        "ws2_32.lib"
        "secur32.lib"
        "bcrypt.lib"
        "crypt32.lib"
        "shlwapi.lib"
        "user32.lib"
        "ole32.lib"
        "advapi32.lib"
    )
    Get-ChildItem $ffmsBuildDir -Filter "*.obj" | ForEach-Object {
        $linkRspLines += "`"$($_.FullName)`""
    }

    $linkRspPath = Join-Path $ffmsBuildDir "link.rsp"
    Set-Content -Path $linkRspPath -Value ($linkRspLines -join "`r`n") -Encoding ASCII

    & link.exe "@$linkRspPath"
    if ($LASTEXITCODE -ne 0) {
        throw "link.exe failed linking ffms2.dll (exit $LASTEXITCODE)"
    }
    Set-Content -Path $ffmsStampPath -Value $desiredStamp -Encoding ASCII
}

# 4b. Download pinned BestSource (R23, MIT) + its two small deps: libp2p (WTFPL, pixel packing)
#     and xxHash (BSD-2-Clause, header-only use). The sources are staged under .native/src and
#     compiled by crates/bestsource-sys/build.rs with the `cc` crate against the FFmpeg above.
$SrcOut = Join-Path $OutputDir "src"
New-Item -ItemType Directory -Force -Path $SrcOut | Out-Null
$bsSources = @(
    @{ Name = "bestsource"; Url = "https://github.com/vapoursynth/bestsource/archive/refs/tags/R23.tar.gz"; Sha = "1CBC9586BD83668BE130B2D69FAA46448D3F6A2E2C075DAE2977A87358E8986B"; Archive = "bestsource-R23.tar.gz"; Dir = "bestsource-R23" },
    @{ Name = "libp2p"; Url = "https://github.com/sekrit-twc/libp2p/archive/869fa993041f9f3af7d9ac8b10158920c6ddce66.tar.gz"; Sha = "CACEF2683A19A8B288CF567544B507F7F06DD66160DB566BF3349E5EE0B73D90"; Archive = "libp2p-869fa993.tar.gz"; Dir = "libp2p-869fa993041f9f3af7d9ac8b10158920c6ddce66" },
    @{ Name = "xxhash"; Url = "https://github.com/Cyan4973/xxHash/archive/refs/tags/v0.8.3.tar.gz"; Sha = "AAE608DFE8213DFD05D909A57718EF82F30722C392344583D3F39050C7F29A80"; Archive = "xxHash-0.8.3.tar.gz"; Dir = "xxHash-0.8.3" }
)
foreach ($s in $bsSources) {
    $archivePath = Join-Path $DownloadsDir $s.Archive
    Invoke-PinnedDownload -Url $s.Url -DestPath $archivePath -ExpectedSha256 $s.Sha
    $dest = Join-Path $SrcOut $s.Name
    $stamp = Join-Path $SrcOut "$($s.Name).stamp"
    $current = if (Test-Path $stamp) { (Get-Content -Raw $stamp).Trim() } else { "" }
    if ($Force -or (-not (Test-Path $dest)) -or ($current -ne $s.Sha)) {
        Write-Host "[native] Extracting $($s.Archive) to $dest ..."
        $extracted = Join-Path $WorkDir $s.Dir
        if (Test-Path $extracted) { Remove-Item -Recurse -Force $extracted }
        if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
        & tar.exe -xf $archivePath -C $WorkDir
        if ($LASTEXITCODE -ne 0) {
            throw "tar.exe failed extracting $archivePath"
        }
        Move-Item $extracted $dest
        Set-Content -Path $stamp -Value $s.Sha -Encoding ASCII
    }
}

# 5. Stage headers, import libraries, runtime DLLs, and licenses into .native/
Write-Host "[native] Staging headers, libs, DLLs, and license notices into $OutputDir ..."

# FFmpeg headers + import libraries (bestsource-sys compiles BestSource against them)
foreach ($dir in @("libavcodec", "libavformat", "libavutil", "libswscale", "libswresample")) {
    $destInc = Join-Path $IncludeOut $dir
    if (Test-Path $destInc) { Remove-Item -Recurse -Force $destInc }
    Copy-Item -Recurse -Force (Join-Path $ffmpegRoot "include\$dir") $destInc
}
foreach ($libName in @("avcodec.lib", "avformat.lib", "avutil.lib", "swscale.lib", "swresample.lib")) {
    Copy-Item -Force (Join-Path $ffmpegRoot "lib\$libName") $LibOut
}

# The same LGPL ffmpeg.exe, used by tests/benches to generate clips (kept off PATH on purpose)
$ToolsOut = Join-Path $OutputDir "tools"
New-Item -ItemType Directory -Force -Path $ToolsOut | Out-Null
Copy-Item -Force (Join-Path $ffmpegRoot "bin\ffmpeg.exe") $ToolsOut
Get-ChildItem (Join-Path $ffmpegRoot "bin") -Filter "*.dll" | ForEach-Object { Copy-Item -Force $_.FullName $ToolsOut }

# Headers
$assIncludeOut = Join-Path $IncludeOut "ass"
New-Item -ItemType Directory -Force -Path $assIncludeOut | Out-Null
Copy-Item -Force (Join-Path $vcpkgTripletDir "include\ass\*.h") $assIncludeOut
Copy-Item -Force (Join-Path $ffmsRoot "include\ffms.h") $IncludeOut
Copy-Item -Force (Join-Path $ffmsRoot "include\ffmscompat.h") $IncludeOut

# Import libraries
Copy-Item -Force (Join-Path $vcpkgTripletDir "lib\ass.lib") $LibOut

# Runtime DLLs from vcpkg (only the DLLs required by ass-9.dll and its dependencies)
$vcpkgDlls = @(
    "ass-9.dll",
    "brotlicommon.dll",
    "brotlidec.dll",
    "bz2.dll",
    "freetype.dll",
    "fribidi-0.dll",
    "harfbuzz.dll",
    "libpng16.dll",
    "libz.dll"
)
foreach ($dllName in $vcpkgDlls) {
    Copy-Item -Force (Join-Path $vcpkgTripletDir "bin\$dllName") $BinOut
}

# Runtime DLLs from FFmpeg (only shared libs needed by ffms2)
$ffmpegDlls = @(
    "avcodec-*.dll",
    "avformat-*.dll",
    "avutil-*.dll",
    "swscale-*.dll",
    "swresample-*.dll"
)
foreach ($pattern in $ffmpegDlls) {
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinOut $pattern)
    Copy-Item -Force (Join-Path $ffmpegRoot "bin\$pattern") $BinOut
}

# License texts
Copy-Item -Force (Join-Path $ffmpegRoot "LICENSE.txt") (Join-Path $LicensesOut "FFmpeg-LGPLv3.txt")
Copy-Item -Force (Join-Path $ffmsRoot "COPYING") (Join-Path $LicensesOut "FFMS2-MIT.txt")
$vcpkgShare = Join-Path $vcpkgTripletDir "share"
if (Test-Path $vcpkgShare) {
    Get-ChildItem $vcpkgShare -Recurse -Filter "copyright" | ForEach-Object {
        $pkgName = $_.Directory.Name
        Copy-Item -Force $_.FullName (Join-Path $LicensesOut "$pkgName-copyright.txt")
    }
}

Write-Host "[native] Done! Staged native dependencies:"
Get-ChildItem $BinOut -Filter "*.dll" | ForEach-Object { Write-Host "  DLL: $($_.Name)" }
Get-ChildItem $LibOut -Filter "*.lib" | ForEach-Object { Write-Host "  LIB: $($_.Name)" }
