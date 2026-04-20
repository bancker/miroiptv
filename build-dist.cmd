@echo off
REM Windows wrapper so `build-dist.cmd --release` works from plain cmd.
REM
REM We skip the mk.cmd chain here because the nested `bash -c "..."`
REM inside mk.cmd mangles quoting badly enough that env-var prefixes
REM get parsed as the script (bash -c TV_VERSION=x ./script interprets
REM the env assignment as $0, not as a leading var for the script).
REM Instead we invoke MSYS2 bash once with one carefully-built command
REM that cmd can't break apart.
REM
REM Version comes from Windows-native git (Git for Windows is already
REM on PATH for day-to-day `git push`); MSYS2's MinGW64 profile doesn't
REM bundle git, so running `git describe` inside the MSYS2 shell fails.
REM
REM Usage:
REM   build-dist.cmd             build miroiptv-<ver>.zip
REM   build-dist.cmd --release   build AND upload to GitHub Releases
REM                              (requires `gh auth login` beforehand)
setlocal

set "MSYS_BASH=C:\Users\brnck\scoop\apps\msys2\current\usr\bin\bash.exe"
if not exist "%MSYS_BASH%" (
    echo [dist] MSYS2 bash not found at %MSYS_BASH% 1>&2
    exit /b 127
)

REM Compute version on the Windows side.
set "TV_VERSION=dev"
for /f "usebackq delims=" %%i in (`git describe --tags --dirty 2^>nul`) do set "TV_VERSION=%%i"
set "TV_REPO=bancker/miroiptv"

echo [dist] version=%TV_VERSION% repo=%TV_REPO%

REM One bash -lc call. cmd expands %TV_VERSION% / %TV_REPO% before
REM handing the string off, so the bash command receives literal values.
REM Exports happen INSIDE bash so build-dist.sh sees them via env.
"%MSYS_BASH%" -lc "export TV_VERSION=%TV_VERSION% TV_REPO=%TV_REPO% MSYSTEM=MINGW64; source /etc/profile; cd '/c/Dropbox/Sources/tv' && ./build-dist.sh"
if errorlevel 1 (
    echo [dist] build failed ^(exit %errorlevel%^) - not uploading
    exit /b 1
)

set "ZIP_PATH=miroiptv-%TV_VERSION%.zip"
if not exist "%ZIP_PATH%" (
    echo [dist] expected %ZIP_PATH% but it wasn't produced - aborting
    exit /b 1
)
echo [dist] produced %ZIP_PATH%

if /i "%~1"=="--release" (
    where gh >nul 2>&1
    if errorlevel 1 (
        echo [dist] gh CLI not found on PATH - install https://cli.github.com or skip --release
        exit /b 1
    )
    echo [dist] creating GitHub release %TV_VERSION% on %TV_REPO%
    gh release create "%TV_VERSION%" "%ZIP_PATH%" ^
        --repo "%TV_REPO%" ^
        --title "miroiptv %TV_VERSION%" ^
        --notes "Portable Windows build. Unzip and run.bat."
)

endlocal
