@echo off
rem =============================================================
rem  Bongo IPTV Player - personal launcher for the Rust release build.
rem
rem  No credentials here: the portal login comes from
rem  BongoIPTVPlayer.ini next to the exe (written by the first-run
rem  prompt, or moved there from BongoPlayer.ini or tvplayer.ini on its first start).
rem  Arguments are passed through, e.g. a bare stream URL or --console.
rem
rem  Build first: cargo build --release (in the rust folder).
rem =============================================================
setlocal

set "EXE=%~dp0rust\target\release\BongoIPTVPlayer.exe"
if not exist "%EXE%" (
    echo BongoIPTVPlayer.exe not found. Build it first: cargo build --release, in the rust folder.
    pause
    exit /b 1
)
start "" "%EXE%" %*
