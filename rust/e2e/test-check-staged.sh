#!/usr/bin/env bash
# Checks check-staged.sh against a made-up ini, in a throw-away git repository:
# the real ini is never read. Exit 0 when every case comes out right.
here="$(cd "$(dirname "$0")" && pwd)"
# A git variable of the caller (a hook's GIT_INDEX_FILE, say) must not point git
# at the caller's repository: only the throw-away one is ever staged into.
unset $(git rev-parse --local-env-vars)
work="$(mktemp -d)" && [ -d "$work" ] || { echo "no temp folder to work in" >&2; exit 1; }
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1
git init -q . || exit 1
git config user.email t@example.com && git config user.name t && git config core.autocrlf false
cat > ini <<'EOF'
portal=alice:Secret-One@portal.example.net:8080
source=1;xtream;;alice:Secret-One@portal.example.net:8080
source=2;xtream;Tweede;bob:Pass;Two@other.example.net:80
source=3;m3u;Betaald;https://carol:Pw-Three@lists.example.net/tv.m3u?token=Tok-Four
source=4;m3u;Gratis;https://iptv-org.github.io/iptv/index.m3u
source=5; XTREAM ;Spaties; dave:Pw-Five@spaces.example.net:81
source=6;M3U;Fragment;https://lists.example.net/tv.m3u?password=Pw-Six#.m3u
source=7;m3u;Leeg;https://lists.example.net/b.m3u?token&x=1
active=1
EOF
fail=0
# check <what> <the exit code it must give> <what is staged>
check() {
  printf '%s\n' "$3" > staged.txt
  git add staged.txt
  BONGO_CHECK_INI="$work/ini" bash "$here/check-staged.sh" > out.txt 2>&1
  local got=$?
  git rm -q --cached staged.txt
  if [ "$got" = "$2" ]; then echo "ok    $1"; else echo "FAIL  $1 (exit $got, wanted $2): $(cat out.txt)"; fail=1; fi
  if grep -q -e Secret-One -e 'Pass;Two' -e Pw-Three -e Tok-Four -e Pw-Five -e Pw-Six out.txt; then
    echo "FAIL  $1: a secret was printed"; fail=1
  fi
  # Counts only: one line of three numbers, which also shows the scan ran.
  if [ "$(grep -Ec '^staged diff: username hits [0-9]+, password hits [0-9]+, token hits [0-9]+$' out.txt)" != 1 ] || [ "$(wc -l < out.txt)" -ne 1 ]; then
    echo "FAIL  $1: the output is not just the counts"; fail=1
  fi
}
check "a clean diff passes, iptv-org's public address too" 0 "nothing here, https://iptv-org.github.io/iptv/index.m3u"
check "the portal line's password" 1 "x Secret-One y"
check "a second portal's password, with a ;" 1 "x Pass;Two y"
check "a second portal's user" 1 "user bob was here"
check "an m3u address's password" 1 "Pw-Three"
check "an m3u address's token" 1 "the Tok-Four token"
check "an m3u address's user" 1 "carol"
check "a kind in capitals and spaces around the fields still count" 1 "dave"
check "a query value that ends at a fragment" 1 "Pw-Six"
check "a query key without a value is no secret" 0 "the word token"
exit $fail
