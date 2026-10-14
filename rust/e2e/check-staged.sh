#!/usr/bin/env bash
# Fails when the staged diff holds a credential from BongoPlayer.ini next to the
# release exe: the user and password of every Xtream portal in it, and of every
# M3U address the user, password and token in it. A public M3U address is no
# secret. Prints counts only, never a value. Run before a commit.
# BONGO_CHECK_INI points it at another ini (test-check-staged.sh does).
INI="${BONGO_CHECK_INI:-$(dirname "$0")/../target/release/BongoPlayer.ini}"
[ -f "$INI" ] || { echo "no $INI - nothing to check against"; exit 0; }
users=(); passes=(); tokens=()
# trim <variable>: the variable without the spaces around its value (the app
# trims every field it reads from the ini; so does the scan).
trim() {
  local s="${!1}"
  s="${s#"${s%%[![:space:]]*}"}"; s="${s%"${s##*[![:space:]]}"}"
  printf -v "$1" '%s' "$s"
}
# An Xtream address: user:pass@host:port (the last @ comes before the host).
xtream() {
  local userpass="${1%@*}"
  [ "$userpass" = "$1" ] && return
  users+=("${userpass%%:*}"); passes+=("${userpass#*:}")
}
# An M3U address: a user:pass@ before its host, and the secrets in its query.
m3u() {
  local rest="${1#*://}" authority userinfo query pair key value
  local -a pairs
  authority="${rest%%[/?#]*}"
  case "$authority" in
    *@*)
      userinfo="${authority%@*}"
      users+=("${userinfo%%:*}")
      case "$userinfo" in *:*) passes+=("${userinfo#*:}") ;; esac ;;
  esac
  case "$1" in
    *\?*)
      query="${1#*\?}"; query="${query%%#*}"
      IFS='&' read -ra pairs <<< "$query"
      for pair in "${pairs[@]}"; do
        case "$pair" in *=*) ;; *) continue ;; esac
        key="${pair%%=*}"; value="${pair#*=}"
        case "${key,,}" in
          username|user) users+=("$value") ;;
          password|pass|token) tokens+=("$value") ;;
        esac
      done ;;
  esac
}
portal_seen=0
while IFS= read -r line || [ -n "$line" ]; do
  line="${line%$'\r'}"; line="${line#$'\xef\xbb\xbf'}"; trim line
  case "$line" in ''|'#'*|';'*) continue ;; esac
  case "$line" in
    source=*)
      rest="${line#source=}"; rest="${rest#*;}"
      kind="${rest%%;*}"; rest="${rest#*;}"; address="${rest#*;}"
      trim kind; kind="${kind,,}"; trim address
      case "$kind" in
        xtream) xtream "$address" ;;
        m3u) m3u "$address" ;;
      esac ;;
    active=*) ;;
    *@*)
      [ "$portal_seen" = 1 ] && continue
      portal_seen=1
      value="$line"
      case "$line" in
        *=*) key="${line%%=*}"; case "$key" in *[@:]*) ;; *) value="${line#*=}" ;; esac ;;
      esac
      trim value; xtream "$value" ;;
  esac
done < "$INI"
diff=$(git diff --cached)
hits() {
  local total=0 secret
  for secret in "$@"; do
    [ "${#secret}" -ge 3 ] || continue
    total=$((total + $(printf '%s' "$diff" | grep -cF -- "$secret")))
  done
  echo "$total"
}
nu=$(hits "${users[@]}"); np=$(hits "${passes[@]}"); nt=$(hits "${tokens[@]}")
echo "staged diff: username hits $nu, password hits $np, token hits $nt"
[ "$nu" = 0 ] && [ "$np" = 0 ] && [ "$nt" = 0 ]
