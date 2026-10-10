#!/usr/bin/env bash
# Wrapper that runs a command inside MSYS2 MINGW64 environment.
# Usage:
#   ./mk                  -> make
#   ./mk test             -> make test
#   ./mk clean            -> make clean
#   ./mk -- <cmd>...      -> run arbitrary command under MINGW64 (args preserved)
#
# Arg-preservation: we printf %q each arg and concatenate, so spaces / quotes
# inside an argument survive being ferried through `bash -lc`. The previous
# $* approach destroyed quoting (see: early tests failing to pass URLs to
# ./build/tv.exe through `./mk -- bash -c '...'`).
set -euo pipefail

MSYS2_ROOT="/c/Users/brnck/scoop/apps/msys2/current"
MSYS_BASH="$MSYS2_ROOT/usr/bin/bash.exe"

if [[ ! -x "$MSYS_BASH" ]]; then
    echo "mk: MSYS2 bash not found at $MSYS_BASH" >&2
    exit 127
fi

# Convert current pwd from Windows style (C:\foo\bar) to MSYS posix (/c/foo/bar).
HERE_WIN="$(pwd -W 2>/dev/null || pwd)"
HERE_POSIX="$(echo "$HERE_WIN" | sed -E -e 's|^([A-Za-z]):|/\L\1|' -e 's|\\|/|g')"

if [[ "${1:-}" == "--" ]]; then
    shift
    prefix=""
else
    prefix="make"
fi

# Build the command string with each arg %q-escaped so whitespace and meta
# characters survive the bash -lc re-parse.
escaped="$prefix"
for arg in "$@"; do
    escaped+=$(printf ' %q' "$arg")
done

exec "$MSYS_BASH" -lc "MSYSTEM=MINGW64 source /etc/profile; cd '$HERE_POSIX' && ${escaped}"
