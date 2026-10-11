#!/usr/bin/env bash
# Fails when the staged diff holds the real portal credentials (read from
# BongoPlayer.ini next to the release exe, never printed). Run before a commit.
INI="$(dirname "$0")/../target/release/BongoPlayer.ini"
[ -f "$INI" ] || { echo "no $INI - nothing to check against"; exit 0; }
line=$(grep '@' "$INI" | head -1 | tr -d '\r')
case "$line" in *=*) creds="${line#*=}" ;; *) creds="$line" ;; esac
creds=$(echo "$creds" | xargs)
userpass="${creds%@*}"
u="${userpass%%:*}"; p="${userpass#*:}"
diff=$(git diff --cached)
nu=$(printf '%s' "$diff" | grep -cF -- "$u"); np=$(printf '%s' "$diff" | grep -cF -- "$p")
echo "staged diff: username hits $nu, password hits $np"
[ "$nu" = 0 ] && [ "$np" = 0 ]
