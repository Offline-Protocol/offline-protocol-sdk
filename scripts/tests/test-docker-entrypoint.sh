#!/usr/bin/env bash

# Pins bindings/python/docker/entrypoint.sh: the command each environment
# gives the service, and the environments it refuses. A stub
# offline-protocol-service on PATH prints its arguments, one per line.
#
# The case that matters most is an explicit flag after the image name: the
# service keeps the last occurrence of a flag, so the operator's arguments
# must come after the environment's, or `--http 0.0.0.0:8080` loses to
# OP_HTTP's loopback default without a word.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENTRYPOINT="$SCRIPT_DIR/../../bindings/python/docker/entrypoint.sh"
FAILURES=0

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/cwd"
cat >"$WORK/bin/offline-protocol-service" <<'EOF'
#!/bin/sh
for arg in "$@"; do
    printf '%s\n' "$arg"
done
EOF
chmod +x "$WORK/bin/offline-protocol-service"

BASE="--config
/etc/offline-protocol/config.json
--mls-root
/var/lib/offline-protocol/mls
--state-root
/var/lib/offline-protocol/state
--socket
/run/offline-protocol/api.sock
--listen
0.0.0.0:7878"

# run NAME WANT [VAR=VALUE ...] [-- ARG ...]: WANT is the argument list one
# per line, or REFUSED for exit status 64.
run() {
  local name="$1" want="$2" got status
  shift 2
  local env_args=() cmd_args=()
  while [ $# -gt 0 ]; do
    if [ "$1" = "--" ]; then
      shift
      cmd_args=("$@")
      break
    fi
    env_args+=("$1")
    shift
  done
  set +e
  # The working directory holds a file the IPv6 entry matches as a bracket
  # pattern, so a glob left on would turn the entry into that file's name.
  got="$(cd "$WORK/cwd" && env -i PATH="$WORK/bin:/usr/bin:/bin" \
    OFFLINE_PROTOCOL_STORE_KEY=00 ${env_args[@]+"${env_args[@]}"} \
    sh "$ENTRYPOINT" ${cmd_args[@]+"${cmd_args[@]}"} 2>/dev/null)"
  status=$?
  set -e
  if [ "$status" -eq 64 ]; then
    got=REFUSED
  elif [ "$status" -ne 0 ]; then
    got="exit $status"
  fi
  if [ "$got" = "$want" ]; then
    echo "  ok - $name"
  else
    echo "  FAIL - $name" >&2
    echo "    got:" >&2
    printf '%s\n' "$got" | sed 's/^/      /' >&2
    echo "    want:" >&2
    printf '%s\n' "$want" | sed 's/^/      /' >&2
    FAILURES=$((FAILURES + 1))
  fi
}

# `[fd00::1]:7878` as a pattern is one of f, d, 0, : or 1, then ":7878".
touch "$WORK/cwd/f:7878"

run "defaults" "$BASE
--lan
--http
127.0.0.1:8080"

run "an explicit flag wins over its variable" "$BASE
--lan
--http
127.0.0.1:8080
--http
0.0.0.0:9090
--http-token-file
/t" -- --http 0.0.0.0:9090 --http-token-file /t

run "an operator argument with a space stays one argument" "$BASE
--lan
--http
127.0.0.1:8080
--http-aliases
/a b.json" -- --http-aliases "/a b.json"

run "peers split on whitespace, never globbed" "$BASE
--peer
[fd00::1]:7878
--peer
off1x@h:1
--lan
--http
127.0.0.1:8080" "OP_PEERS=[fd00::1]:7878 off1x@h:1"

run "OP_LAN=0 leaves LAN discovery off" "$BASE
--http
127.0.0.1:8080" OP_LAN=0

run "an empty OP_HTTP starts no front" "$BASE
--lan" OP_HTTP=

run "the front's flags follow OP_HTTP" "$BASE
--lan
--http
0.0.0.0:8080
--http-token-file
/run/front/token
--http-aliases
/etc/aliases.json
--relay
wss://relay.example" OP_HTTP=0.0.0.0:8080 OP_HTTP_TOKEN_FILE=/run/front/token \
  OP_HTTP_ALIASES=/etc/aliases.json OP_RELAY=wss://relay.example

run "OP_LAN other than 1 or 0 is refused" REFUSED OP_LAN=2
run "an empty OP_LAN is refused" REFUSED OP_LAN=
run "a front flag without a front is refused" REFUSED OP_HTTP= OP_HTTP_TOKEN_FILE=/t
run "no store key is refused" REFUSED OFFLINE_PROTOCOL_STORE_KEY=

if [ "$FAILURES" -eq 0 ]; then
  echo "All entrypoint cases passed."
else
  exit 1
fi
