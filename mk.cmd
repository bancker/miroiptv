@echo off
REM Windows CMD wrapper that forwards all args to MSYS2's bash-based ./mk script.
REM Lets you run: `mk -- ./build/tv.exe --xtream user:pass@host:port` from plain cmd.
setlocal
set "MSYS_BASH=C:\Users\brnck\scoop\apps\msys2\current\usr\bin\bash.exe"
if not exist "%MSYS_BASH%" (
    echo mk.cmd: MSYS2 bash not found at %MSYS_BASH% 1>&2
    exit /b 127
)
"%MSYS_BASH%" -lc "MSYSTEM=MINGW64 source /etc/profile; cd '/c/Dropbox/Sources/tv' && ./mk %*"
endlocal
