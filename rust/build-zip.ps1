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
$stagingName = "BongoIPTVPlayer-v$ver"
$staging = Join-Path $distDir $stagingName

if (-not (Test-Path $distDir)) { New-Item -ItemType Directory -Path $distDir -Force | Out-Null }
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging -Force | Out-Null

Copy-Item (Join-Path $rustDir 'target\release\BongoIPTVPlayer.exe') $staging
Copy-Item (Join-Path $rustDir 'vendor\libmpv\libmpv-2.dll') $staging
$mpvDir = Join-Path $rustDir 'vendor\libmpv'
if (Test-Path (Join-Path $mpvDir 'LICENSE.txt')) {
    Copy-Item (Join-Path $mpvDir 'LICENSE.txt') (Join-Path $staging 'libmpv-LICENSE.txt')
}

$runBat = @'
@echo off
REM Starts Bongo IPTV Player. Nothing to edit here: on the first start the app asks
REM for your language and then for your portal (server, user, password), and
REM keeps the portal in BongoIPTVPlayer.ini next to the exe. Change it later in the
REM settings (press o). Anything typed after run.bat goes on to the app.
"%~dp0BongoIPTVPlayer.exe" %*
'@
Set-Content -Path (Join-Path $staging 'run.bat') -Value $runBat -Encoding ASCII

$readme = @"
Bongo IPTV Player v$ver - portable

USAGE
-----
1. Double-click run.bat (or BongoIPTVPlayer.exe).
2. First start: choose your language, then enter your Xtream portal (server,
   user, password) when asked. The app tests it and keeps it in
   BongoIPTVPlayer.ini next to the exe. Or press the link under it to add an M3U
   list (a URL or a file) in the settings.
3. Change the language, the sources (Xtream portals, M3U lists) and more later
   in the settings (press o).

OR from the command line:
  BongoIPTVPlayer.exe --xtream user:pass@host:port    (beats the saved portal for
                                                       as long as it is given;
                                                       the settings say so)
  BongoIPTVPlayer.exe https://example.com/some.m3u8   (bare URL, no portal)
  BongoIPTVPlayer.exe --selftest                      (smoke check)

UPGRADING FROM BONGO PLAYER OR TVPLAYER
---------------------------------------
Favorites, resume points, presets and saved lists move over automatically
on the first start; the old data folder stays as a backup. Your sources
(BongoPlayer.ini, or tvplayer.ini) move too if you unzip into your old
folder; otherwise copy that ini next to BongoIPTVPlayer.exe. From then on
start BongoIPTVPlayer.exe or run.bat: an old BongoPlayer.exe finds no ini.

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
  g                                  TV guide (Xtream) / channel list (M3U)
  a / s                              next audio track / subtitles
  + / -                              higher / lower quality (live)
  left / right arrow                 seek -30s / +30s (movies, catch-up)
  space                              pause / play (movies, series, catch-up)
  o                                  settings (language, sources, adult content, news)
  ?                                  all keys
  F11                                fullscreen
  t                                  borderless + always on top
  right mouse button + drag          move the borderless window
  F5                                 fetch the portal or the list again
  d                                  debug info
  Esc                                close overlays

LICENSE
-------
Bongo IPTV Player code: MIT
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
