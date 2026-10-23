#!/usr/bin/env bash
# Runs every browser test page in Chromium, Firefox and WebKit, headless,
# through Playwright. Builds the module first only when bindings/web/pkg is
# missing: after a Rust change, run scripts/build.sh (CI always does, in the
# step before this one).
#
# BROWSERS narrows the engines (default: chromium firefox webkit). The
# browsers are Playwright's own builds, pinned by the playwright version in
# package-lock.json; `npx playwright install` fetches them. A static server
# on PORT serves this directory, which is all a page, its worker and the
# module need.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-8765}"
# The local Nostr relay the signalling page publishes through.
RELAY_PORT="${RELAY_PORT:-8766}"

[ -f pkg/offline_protocol_web.js ] || bash scripts/build.sh
[ -d node_modules/playwright ] || npm ci

python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1 &
server=$!
node test/nostr-relay.mjs "$RELAY_PORT" >/dev/null 2>&1 &
relay=$!
cleanup() {
  kill "$server" "$relay" 2>/dev/null || true
  wait "$server" "$relay" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break
  sleep 0.2
done

pages=()
for page in test/*.html; do pages+=("$page"); done
node test/run.mjs "http://127.0.0.1:$PORT" "ws://127.0.0.1:$RELAY_PORT" 90000 "${pages[@]}"
