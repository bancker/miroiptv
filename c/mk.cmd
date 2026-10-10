@echo off
REM Windows CMD wrapper that forwards all args to MSYS2's bash-based ./mk script.
REM Lets you run: `mk -- ./build/tv.exe --xtream user:pass@host:port` from plain cmd.
setlocal
set "MSYS_BASH=C:\Users\brnck\scoop\apps\msys2\current\usr\bin\bash.exe"
if not exist "%MSYS_BASH%" (
    echo mk.cmd: MSYS2 bash not found at %MSYS_BASH% 1>&2
    exit /b 127
)
REM cd to this script's own folder (the C project root), wherever the repo
REM lives: the exe loads assets/ relative to the working directory. Forward
REM slashes because MSYS2 bash accepts C:/... paths as-is.
set "HERE=%~dp0"
set "HERE=%HERE:\=/%"
"%MSYS_BASH%" -lc "MSYSTEM=MINGW64 source /etc/profile; cd '%HERE%' && ./mk %*"
endlocal
