#!/usr/bin/env bash
# Wrapper that runs any command inside MSYS2 MINGW64 environment.
# Usage:
#   ./mk                -> make
#   ./mk test           -> make test
#   ./mk clean          -> make clean
#   ./mk run            -> make run
#   ./mk -- <cmd...>    -> run arbitrary command under MINGW64
#
# Exists so subagents don't need to know the MSYS2 install path.
set -euo pipefail

MSYS2_ROOT="/c/Users/brnck/scoop/apps/msys2/current"
MSYS_BASH="$MSYS2_ROOT/usr/bin/bash.exe"

if [[ ! -x "$MSYS_BASH" ]]; then
    echo "mk: MSYS2 bash not found at $MSYS_BASH" >&2
    exit 127
fi

if [[ "${1:-}" == "--" ]]; then
    shift
    cmd="$*"
else
    cmd="make $*"
fi

# Convert Windows-style pwd (C:\foo\bar) to MSYS posix (/c/foo/bar).
HERE_WIN="$(pwd -W 2>/dev/null || pwd)"
HERE_POSIX="$(echo "$HERE_WIN" | sed -E -e 's|^([A-Za-z]):|/\L\1|' -e 's|\\|/|g')"

exec "$MSYS_BASH" -lc "MSYSTEM=MINGW64 source /etc/profile; cd '$HERE_POSIX' && $cmd"
