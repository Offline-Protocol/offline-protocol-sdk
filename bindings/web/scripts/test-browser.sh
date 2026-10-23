#!/usr/bin/env bash
# Runs the browser test page in headless Chrome against a fresh build.
#
# CHROME names the browser binary; the default is the macOS install path, and
# CI sets it to the runner's Chrome. A static server on PORT serves this
# directory, which is all a page, its worker and the module need.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
PORT="${PORT:-8765}"
DEVTOOLS_PORT="${DEVTOOLS_PORT:-9333}"

[ -f pkg/offline_protocol_web.js ] || bash scripts/build.sh

profile="$(mktemp -d)"
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1 &
server=$!
# CHROME_FLAGS carries what a CI sandbox needs (--no-sandbox on Linux runners).
# shellcheck disable=SC2086
"$CHROME" ${CHROME_FLAGS:-} --headless=new --disable-gpu --no-first-run --no-default-browser-check \
  --user-data-dir="$profile" --remote-debugging-port="$DEVTOOLS_PORT" about:blank \
  >/dev/null 2>&1 &
chrome=$!
cleanup() {
  kill "$server" "$chrome" 2>/dev/null || true
  wait "$server" "$chrome" 2>/dev/null || true
  # Chrome's helper processes can still be writing the profile as it exits.
  rm -rf "$profile" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$DEVTOOLS_PORT/json/version" >/dev/null 2>&1 && break
  sleep 0.2
done

status=0
for page in test/*.html; do
  echo "== $page"
  node test/run.mjs "$DEVTOOLS_PORT" "http://127.0.0.1:$PORT/$page" 90000 || status=1
done
exit $status
