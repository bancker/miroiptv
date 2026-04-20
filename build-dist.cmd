@echo off
REM Windows wrapper so `build-dist.cmd --release` works from plain cmd.
REM
REM Compute version via system git (MSYS2's bundled git may be missing;
REM Git for Windows is on user PATH), then forward the *build* step into
REM MSYS2 MinGW64 via mk.cmd so gcc/ldd resolve. The optional `gh release`
REM upload runs from this Windows shell afterwards — also because gh is
REM on Windows PATH, not MSYS2's.
REM
REM Usage:
REM   build-dist.cmd             build miroiptv-<ver>.zip
REM   build-dist.cmd --release   build AND upload to GitHub Releases
REM                              (requires `gh auth login` beforehand)
setlocal

REM Compute version from git — fall back to "dev" if anything goes wrong.
set "TV_VERSION=dev"
for /f "usebackq delims=" %%i in (`git describe --tags --dirty 2^>nul`) do set "TV_VERSION=%%i"

set "TV_REPO=bancker/miroiptv"

echo [dist] version=%TV_VERSION% repo=%TV_REPO%

REM Run the build inside MSYS2 — env vars propagate to bash via the
REM login-shell invocation mk.cmd sets up.
call "%~dp0mk.cmd" -- bash -c "TV_VERSION=%TV_VERSION% TV_REPO=%TV_REPO% ./build-dist.sh"
if errorlevel 1 (
    echo [dist] build failed — not uploading
    exit /b 1
)

set "ZIP_PATH=miroiptv-%TV_VERSION%.zip"
if not exist "%ZIP_PATH%" (
    echo [dist] expected %ZIP_PATH% but it wasn't produced — aborting
    exit /b 1
)
echo [dist] produced %ZIP_PATH%

REM --release? Upload via gh. gh runs from Windows PATH (outside MSYS2).
if /i "%~1"=="--release" (
    where gh >nul 2>&1
    if errorlevel 1 (
        echo [dist] gh CLI not found on PATH — install https://cli.github.com or skip --release
        exit /b 1
    )
    echo [dist] creating GitHub release %TV_VERSION% on %TV_REPO%
    gh release create "%TV_VERSION%" "%ZIP_PATH%" ^
        --repo "%TV_REPO%" ^
        --title "miroiptv %TV_VERSION%" ^
        --notes "Portable Windows build. Unzip and run.bat."
)

endlocal
