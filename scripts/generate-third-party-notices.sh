#!/usr/bin/env bash
# Regenerates THIRD-PARTY-NOTICES.md from the shipped-binary dependency graph
# (the offline-protocol-uniffi crate — what actually links into the iOS/Android
# libraries and the Python wheel's native library) and copies it to every
# package that redistributes those binaries. Run after dependency changes.
# The notices job in ci.yml reruns this and fails on any diff, so a missed
# regeneration no longer ships.
set -euo pipefail
cd "$(dirname "$0")/.."

# Pinned because output formatting can change across cargo-about releases,
# which the CI drift gate would misread as stale attribution. ci.yml reads
# this assignment with sed; keep the NAME="value" shape on its own line.
CARGO_ABOUT_VERSION="0.9.1"

installed="$(cargo about --version 2>/dev/null || true)"
if [ "$installed" != "cargo-about $CARGO_ABOUT_VERSION" ]; then
  echo "error: cargo-about $CARGO_ABOUT_VERSION required (found: ${installed:-none})" >&2
  echo "       cargo install cargo-about --version $CARGO_ABOUT_VERSION --features cli --locked" >&2
  exit 1
fi

cargo about generate \
  --manifest-path crates/offline-protocol-uniffi/Cargo.toml \
  --locked \
  -o THIRD-PARTY-NOTICES.md \
  about.hbs

cp THIRD-PARTY-NOTICES.md bindings/react-native/THIRD-PARTY-NOTICES.md
cp THIRD-PARTY-NOTICES.md bindings/python/THIRD-PARTY-NOTICES.md

echo "Regenerated THIRD-PARTY-NOTICES.md (root, react-native, python)."
