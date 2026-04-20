@echo off
REM Windows wrapper so `build-dist.cmd --release` works from plain cmd.
REM Forwards the invocation into MSYS2 MinGW64 (via mk.cmd) so gcc,
REM make, ldd, and the /mingw64/bin/ DLL tree are all on PATH.
REM
REM Usage (from cmd or PowerShell):
REM   build-dist.cmd             build miroiptv-<ver>.zip
REM   build-dist.cmd --release   build AND upload to GitHub Releases
REM                              (requires `gh auth login` beforehand)

call "%~dp0mk.cmd" -- bash -c "./build-dist.sh %*"
