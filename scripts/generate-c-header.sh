#!/usr/bin/env bash
# Writes the C binding's header, bindings/c/include/offline_protocol.h, from
# the Rust in crates/offline-protocol-uniffi/src/capi.
#
#   scripts/generate-c-header.sh           write the header
#   scripts/generate-c-header.sh --check   fail when the committed header is stale
#
# The cbindgen version is pinned because its output formatting changes across
# releases, which would read as drift; CI reads the pin from this line.
set -euo pipefail
cd "$(dirname "$0")/.."

CBINDGEN_VERSION="0.29.4"
CRATE_DIR="crates/offline-protocol-uniffi"
HEADER="bindings/c/include/offline_protocol.h"

installed="$(cbindgen --version 2>/dev/null || true)"
if [ "$installed" != "cbindgen $CBINDGEN_VERSION" ]; then
  echo "error: cbindgen $CBINDGEN_VERSION required (found: ${installed:-none})" >&2
  echo "       cargo install cbindgen --version $CBINDGEN_VERSION --locked" >&2
  exit 1
fi

# cbindgen's warnings for the crate's Rust-only constants are expected; its
# errors still fail the run through `set -e`. So is one per item about the
# `capi` feature: the module is behind it, and a `[defines]` entry would wrap
# every declaration in a preprocessor guard a C host would have to define.
# The header describes the library that has the feature; one without it has
# no such symbols to declare.
generate() {  # generate <output path>
  cbindgen --config "$CRATE_DIR/cbindgen.toml" --crate offline-protocol-uniffi \
    --lang c --output "$1" "$CRATE_DIR" \
    2> >(grep -v -e '^WARN: Skip ' -e '^WARN: Missing `\[defines\]` entry for `feature = "capi"`' >&2)
}

case "${1:-}" in
  "")
    mkdir -p "$(dirname "$HEADER")"
    generate "$HEADER"
    echo "wrote $HEADER"
    ;;
  --check)
    # Into a scratch file and compared, never `cbindgen --verify`: that flag
    # reports a difference and also rewrites the file it compared against, so
    # a check would leave the stale header replaced and the next run green.
    fresh="$(mktemp)"
    trap 'rm -f "$fresh"' EXIT
    generate "$fresh"
    if ! cmp -s "$fresh" "$HEADER"; then
      diff -u "$HEADER" "$fresh" >&2 || true
      echo "error: $HEADER is stale against $CRATE_DIR/src/capi." >&2
      echo "       Run scripts/generate-c-header.sh and commit the result." >&2
      exit 1
    fi
    ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac
