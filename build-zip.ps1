# Build release exe + bundle libmpv-2.dll + assets into a portable zip.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$rustDir = Join-Path $root 'rust'
$distDir = Join-Path $root 'dist'

if (-not (Test-Path (Join-Path $rustDir 'vendor\libmpv\libmpv-2.dll'))) {
    Write-Host 'libmpv not vendored - running fetch-mpv.ps1'
    & (Join-Path $rustDir 'vendor\fetch-mpv.ps1')
}

Push-Location $rustDir
try {
    Write-Host '== cargo build --release =='
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
} finally { Pop-Location }

$verLine = Select-String -Path (Join-Path $rustDir 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $verLine) { throw "could not parse version from Cargo.toml" }
$ver = $verLine.Matches.Groups[1].Value
$stagingName = "tvplayer-v$ver"
$staging = Join-Path $distDir $stagingName

if (-not (Test-Path $distDir)) { New-Item -ItemType Directory -Path $distDir -Force | Out-Null }
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging -Force | Out-Null

Copy-Item (Join-Path $rustDir 'target\release\tvplayer.exe') $staging
Copy-Item (Join-Path $rustDir 'vendor\libmpv\libmpv-2.dll') $staging
$mpvDir = Join-Path $rustDir 'vendor\libmpv'
if (Test-Path (Join-Path $mpvDir 'LICENSE.txt')) {
    Copy-Item (Join-Path $mpvDir 'LICENSE.txt') (Join-Path $staging 'libmpv-LICENSE.txt')
}

$runBat = @'
@echo off
REM =====================================================================
REM  !!  EDIT THE LINE BELOW WITH YOUR REAL XTREAM PORTAL CREDENTIALS  !!
REM  Format: user:pass@host:port
REM  Without a real portal the app shows a "no portal configured" screen.
REM =====================================================================
set XTREAM_CREDS=user:pass@host.example.com:8080

"%~dp0tvplayer.exe" --xtream %XTREAM_CREDS% %*
'@
Set-Content -Path (Join-Path $staging 'run.bat') -Value $runBat -Encoding ASCII

$readme = @"
tvplayer v$ver - portable

USAGE
-----
1. Edit run.bat: replace the XTREAM_CREDS line with your portal
   user:pass@host:port
2. Double-click run.bat

OR from the command line:
  tvplayer.exe --xtream user:pass@host:port
  tvplayer.exe https://example.com/some.m3u8      (bare URL, no portal)
  tvplayer.exe --selftest                          (smoke check)

KEYS
----
  arrow up/down, mouse wheel        zap previous/next channel
  1 / 2 / 3                          NPO 1 / 2 / 3
  n                                  news shortcut (NPO)
  r                                  news shortcut (RTL)
  f                                  cross-catalog search
  e                                  EPG strip (now + next)
  Shift+E                            EPG grid
  Shift+F                            favorites list
  *                                  toggle favorite for current channel
  a / s                              cycle audio / subtitle track
  + / -                              higher / lower quality (live)
  left / right arrow                 seek -30s / +30s (VOD)
  F11                                fullscreen
  d                                  debug HUD
  Esc                                dismiss overlays

LICENSE
-------
tvplayer code: MIT
libmpv-2.dll : LGPL-2.1 (see libmpv-LICENSE.txt if bundled)

mpv source:    https://github.com/mpv-player/mpv
mpv-dev build: https://github.com/shinchiro/mpv-winbuild-cmake
"@
Set-Content -Path (Join-Path $staging 'README.txt') -Value $readme -Encoding ASCII

$zipPath = Join-Path $distDir "$stagingName.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }

Write-Host "== zipping =="
Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $zipPath -CompressionLevel Optimal

$zipMb = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)
Write-Host ""
Write-Host "BUILT: $zipPath ($zipMb MB)"
Write-Host "  staging: $staging"
Get-ChildItem $staging | Select-Object Name, @{N='Size (MB)'; E={[math]::Round($_.Length / 1MB, 2)}} | Format-Table
