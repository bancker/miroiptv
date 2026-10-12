# Build release exe + bundle libmpv-2.dll + assets into a portable zip.
# Lives in the rust/ project folder; all output stays inside it (dist\).
$ErrorActionPreference = 'Stop'

$rustDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$distDir = Join-Path $rustDir 'dist'

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
$stagingName = "BongoPlayer-v$ver"
$staging = Join-Path $distDir $stagingName

if (-not (Test-Path $distDir)) { New-Item -ItemType Directory -Path $distDir -Force | Out-Null }
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging -Force | Out-Null

Copy-Item (Join-Path $rustDir 'target\release\BongoPlayer.exe') $staging
Copy-Item (Join-Path $rustDir 'vendor\libmpv\libmpv-2.dll') $staging
$mpvDir = Join-Path $rustDir 'vendor\libmpv'
if (Test-Path (Join-Path $mpvDir 'LICENSE.txt')) {
    Copy-Item (Join-Path $mpvDir 'LICENSE.txt') (Join-Path $staging 'libmpv-LICENSE.txt')
}

$runBat = @'
@echo off
REM Starts Bongo Player. Nothing to edit here: on the first start the app asks
REM for your language and then for your portal (server, user, password), and
REM keeps the portal in BongoPlayer.ini next to the exe. Change it later in the
REM settings (press o). Anything typed after run.bat goes on to the app.
"%~dp0BongoPlayer.exe" %*
'@
Set-Content -Path (Join-Path $staging 'run.bat') -Value $runBat -Encoding ASCII

$readme = @"
Bongo Player v$ver - portable

USAGE
-----
1. Double-click run.bat (or BongoPlayer.exe).
2. First start: choose your language, then enter your Xtream portal (server,
   user, password) when asked. The app tests it and keeps it in
   BongoPlayer.ini next to the exe.
3. Change the language, the portal and more later in the settings (press o).

OR from the command line:
  BongoPlayer.exe --xtream user:pass@host:port      (beats the saved portal for
                                                      as long as it is given;
                                                      the settings say so)
  BongoPlayer.exe https://example.com/some.m3u8      (bare URL, no portal)
  BongoPlayer.exe --selftest                          (smoke check)

UPGRADING FROM TVPLAYER
-----------------------
Favorites, resume points and presets move over automatically on the
first start. Your portal login (tvplayer.ini) moves too if you unzip
into your old tvplayer folder; otherwise copy tvplayer.ini next to
BongoPlayer.exe.

KEYS
----
  arrow up/down, mouse wheel        previous / next channel
  0-9 (tap)                          recall a preset channel
  0-9 (hold)                         store the current channel as a preset
  n                                  latest news (choose a bulletin)
  f                                  search (live, movies, series)
  Shift+F                            favorites list
  *                                  toggle favorite for the current channel
  e / Shift+E                        guide strip (now + next) / guide overview
  g                                  TV guide
  a / s                              next audio track / subtitles
  + / -                              higher / lower quality (live)
  left / right arrow                 seek -30s / +30s (movies, catch-up)
  space                              pause / play (movies, series, catch-up)
  o                                  settings (language, portal, adult content, news)
  ?                                  all keys
  F11                                fullscreen
  t                                  borderless + always on top
  F5                                 retry the portal fetch
  d                                  debug info
  Esc                                close overlays

LICENSE
-------
Bongo Player code: MIT
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
