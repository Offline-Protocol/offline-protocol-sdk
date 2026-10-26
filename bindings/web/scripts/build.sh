#!/usr/bin/env bash
# Builds the engine for the browser and writes the module and its JavaScript
# glue to bindings/web/pkg.
#
# wasm-bindgen's command-line tool must be exactly the version of the
# `wasm-bindgen` crate in Cargo.lock: the two share an unversioned schema, and
# a mismatch fails here with a schema error rather than later in a browser.
# The crate pins it with `=`, so the version below moves only with that pin.
set -euo pipefail
cd "$(dirname "$0")/../../.."

WASM_BINDGEN_VERSION="0.2.105"
PROFILE="${PROFILE:-release}"

locked="$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p')"
if [ "$locked" != "$WASM_BINDGEN_VERSION" ]; then
  echo "error: Cargo.lock has wasm-bindgen $locked but this script pins $WASM_BINDGEN_VERSION" >&2
  exit 1
fi
installed="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$installed" != "$WASM_BINDGEN_VERSION" ]; then
  echo "error: wasm-bindgen ${installed:-is not installed}; install the matching CLI:" >&2
  echo "       cargo install wasm-bindgen-cli --version $WASM_BINDGEN_VERSION --locked" >&2
  exit 1
fi

cargo build --locked --package offline-protocol-web --target wasm32-unknown-unknown \
  --profile "$PROFILE"
dir="$PROFILE"
[ "$PROFILE" = dev ] && dir=debug
wasm-bindgen --target web --out-dir bindings/web/pkg \
  "target/wasm32-unknown-unknown/$dir/offline_protocol_web.wasm"

# The id the lobby's cache is named by, over the module just written (W15).
bash bindings/web/scripts/build-id.sh

size="$(wc -c < bindings/web/pkg/offline_protocol_web_bg.wasm | tr -d ' ')"
gz="$(gzip -9 -c bindings/web/pkg/offline_protocol_web_bg.wasm | wc -c | tr -d ' ')"
echo "bindings/web/pkg: module $size bytes, $gz gzipped"
