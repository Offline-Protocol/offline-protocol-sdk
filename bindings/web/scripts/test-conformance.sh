#!/usr/bin/env bash
# Runs both storage conformance suites, and a reopen, against the log store
# over real OPFS files in a dedicated worker of each browser named in
# BROWSERS (default: chrome firefox). The test is
# crates/offline-protocol-web/tests/browser_conformance.rs, run by
# wasm-bindgen's test runner, which drives each browser through WebDriver.
#
# CHROMEDRIVER and GECKODRIVER name the drivers; each must match its browser's
# major version. GitHub's runners carry both (CI points these at them). A
# browser whose driver is not set is an error, never a skip: a run that
# quietly covered one browser would read as covering two.
set -euo pipefail
cd "$(dirname "$0")/../../.."

BROWSERS="${BROWSERS:-chrome firefox}"

# The runner ships with wasm-bindgen-cli, so it is the build script's version.
runner="$(command -v wasm-bindgen-test-runner || true)"
if [ -z "$runner" ]; then
  echo "error: wasm-bindgen-test-runner is not installed; it comes with wasm-bindgen-cli:" >&2
  echo "       cargo install wasm-bindgen-cli --version 0.2.105 --locked" >&2
  exit 1
fi

status=0
for browser in $BROWSERS; do
  case "$browser" in
    chrome) var=CHROMEDRIVER ;;
    firefox) var=GECKODRIVER ;;
    *) echo "error: unknown browser $browser (chrome or firefox)" >&2; exit 1 ;;
  esac
  driver="${!var:-}"
  if [ -z "$driver" ] || [ ! -x "$driver" ]; then
    echo "error: $var must name the $browser WebDriver binary (got '${driver}')" >&2
    exit 1
  fi
  echo "== $browser"
  # Only this browser's driver is in the environment: the runner takes the
  # first one it finds, and a Mac has safaridriver on PATH. A driver found on
  # PATH would quietly change which browser ran.
  env -u CHROMEDRIVER -u GECKODRIVER -u SAFARIDRIVER "$var=$driver" \
    CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="$runner" \
    WASM_BINDGEN_TEST_TIMEOUT="${WASM_BINDGEN_TEST_TIMEOUT:-120}" \
    cargo test --locked --package offline-protocol-web --target wasm32-unknown-unknown \
    --test browser_conformance || status=1
done
exit $status
