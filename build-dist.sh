#!/usr/bin/env bash
# Build a portable Windows distribution zip of miroiptv.exe.
#
# Output: miroiptv-<version>.zip in the project root, containing:
#   miroiptv.exe                 the player
#   *.dll                  every runtime library pulled from /mingw64/bin
#   assets/DejaVuSans.ttf  the font used by overlays
#   run.bat                a launcher the end user double-clicks
#
# Version string: `git describe --tags --dirty`, baked into miroiptv.exe via the
# Makefile's TV_VERSION define so the in-app update check can compare
# against the latest release on GitHub.
#
# Usage:
#   ./build-dist.sh                        # just build the zip
#   ./build-dist.sh --release              # build AND `gh release create`
#   TV_REPO=user/repo ./build-dist.sh      # override default repo slug
set -euo pipefail

REPO="${TV_REPO:-brnck/miroiptv}"
VERSION="$(git describe --tags --dirty 2>/dev/null || echo dev)"
DIST_DIR="dist/miroiptv-${VERSION}"
ZIP_PATH="miroiptv-${VERSION}.zip"
DO_RELEASE=0
for arg in "$@"; do
    [[ "$arg" == "--release" ]] && DO_RELEASE=1
done

echo "=> Building version ${VERSION} (repo ${REPO})"

# 1. Rebuild with the version + repo slug baked in. We use ./mk so this
#    runs under the same MSYS2/MINGW64 env where the dependency DLLs
#    actually live. `make -B` forces recompilation even when object files
#    exist — we want the TV_VERSION / TV_REPO -D flags to take effect,
#    and gmake doesn't track CFLAGS changes automatically. We don't
#    `make clean` because that fights a running build/miroiptv.exe on Windows
#    (handle held open → Device busy), and we just need the .o files
#    rebuilt, not the whole build directory wiped.
./mk -- bash -c "TV_VERSION='${VERSION}' TV_REPO='${REPO}' make -B"

# 2. Fresh dist folder.
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}"
cp build/miroiptv.exe "${DIST_DIR}/"
cp -r assets "${DIST_DIR}/"

# 3. Collect runtime DLLs. `ldd` lists everything the exe (and, transitively,
#    its DLLs) pulls in. We filter to /mingw64/bin/ because Windows system
#    DLLs (ntdll, KERNEL32, ...) are already present on every machine and
#    shipping them would confuse the loader. Iterate two levels deep so
#    the DLLs of DLLs also come along (e.g., avcodec needs swresample
#    which needs avutil — ldd -r recursive behaviour handles that).
echo "=> Collecting DLLs via ldd"
./mk -- bash -c '
set -e
mkdir -p "'"${DIST_DIR}"'/_bundled"
# First pass: direct deps of miroiptv.exe.
ldd build/miroiptv.exe | awk "/=>/ {print \$3}" | grep -iE "/mingw64/bin/" | sort -u > "'"${DIST_DIR}"'/_bundled/deps.txt"
# Recursive: transitively include DLLs of DLLs until the set stabilises.
while : ; do
    new=0
    while read -r dll; do
        [[ -z "$dll" ]] && continue
        [[ -f "$dll" ]] || continue
        cp -n "$dll" "'"${DIST_DIR}"'/"
        ldd "$dll" | awk "/=>/ {print \$3}" | grep -iE "/mingw64/bin/" | while read -r t; do
            grep -qxF "$t" "'"${DIST_DIR}"'/_bundled/deps.txt" || { echo "$t" >> "'"${DIST_DIR}"'/_bundled/deps.txt"; new=1; }
        done
    done < "'"${DIST_DIR}"'/_bundled/deps.txt"
    # The subshell resets $new, so we test via wc -l stability.
    now=$(wc -l < "'"${DIST_DIR}"'/_bundled/deps.txt")
    [[ -z "${prev:-}" ]] && prev=0
    if [[ "$now" == "$prev" ]]; then break; fi
    prev="$now"
done
rm -rf "'"${DIST_DIR}"'/_bundled"
'

# 4. run.bat — double-click launcher. The user edits the first line to
#    paste their Xtream credentials so they don't have to type them every
#    time. If they pass their own args, %* forwards them through.
cat > "${DIST_DIR}/run.bat" <<'BAT'
@echo off
REM Put your Xtream credentials between the quotes (or leave blank to
REM pass them on the command line):
set XTREAM_CREDS=user:pass@host:port

cd /d "%~dp0"
if "%~1"=="" (
    miroiptv.exe --xtream "%XTREAM_CREDS%"
) else (
    miroiptv.exe %*
)
if errorlevel 1 pause
BAT

# 5. README snippet inside the zip so recipients know what to do.
cat > "${DIST_DIR}/README.txt" <<EOF
miroiptv ${VERSION}

Double-click run.bat. Edit it first so XTREAM_CREDS points to your
portal (user:pass@host:port), or pass arguments on the command line.

Keys (press ? in the app for the full list):
  wheel / up / down   zap through channels
  1 / 2 / 3           NPO 1 / 2 / 3
  f                   search channels + movies + series
  e                   EPG overlay    (Shift+e for full multi-day guide)
  n / r               latest NOS Journaal / RTL Nieuws
  a                   cycle audio track    s  cycle subtitles
  space               pause             F11  fullscreen
  t                   always-on-top      q  quit

Updates: ${REPO} — the app checks github.com/${REPO}/releases for a
newer version on startup and flashes a toast when one exists.
EOF

# 6. Zip it. Prefer 7z / InfoZIP / PowerShell in that order — MSYS2 doesn't
#    ship `zip` in the base install, but `7z` via scoop or PowerShell's
#    Compress-Archive are always present on Windows. The three produce
#    functionally equivalent .zip archives for Windows Explorer and gh
#    release upload.
rm -f "${ZIP_PATH}"
if command -v 7z >/dev/null; then
    ( cd dist && 7z a -tzip -bd -bso0 -bsp0 "../${ZIP_PATH}" "miroiptv-${VERSION}" )
elif command -v zip >/dev/null; then
    ( cd dist && zip -qr "../${ZIP_PATH}" "miroiptv-${VERSION}" )
else
    # Fall back to PowerShell — Compress-Archive chokes on paths with
    # spaces when called from MSYS bash because it re-splits argv on
    # spaces, so quote carefully and pass absolute paths.
    powershell -NoProfile -Command \
        "Compress-Archive -Force -Path 'dist\\miroiptv-${VERSION}\\*' -DestinationPath '${ZIP_PATH}'"
fi
BYTES=$(wc -c < "${ZIP_PATH}")
echo "=> ${ZIP_PATH}  ($((BYTES / 1024 / 1024)) MiB)"

# 7. Optionally upload as a GitHub Release. Requires `gh auth login` to
#    have been done at least once. If the tag matches VERSION we attach
#    to that, otherwise gh will create a new one from HEAD.
if [[ "${DO_RELEASE}" == "1" ]]; then
    if ! command -v gh >/dev/null; then
        echo "!! gh CLI not found — skipping upload"
    else
        echo "=> Creating GitHub release ${VERSION} on ${REPO}"
        gh release create "${VERSION}" "${ZIP_PATH}" \
            --repo "${REPO}" \
            --title "miroiptv ${VERSION}" \
            --notes "Portable Windows build. Unzip and run.bat."
    fi
fi
