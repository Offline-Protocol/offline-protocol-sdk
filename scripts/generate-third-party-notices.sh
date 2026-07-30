#!/usr/bin/env bash
# Regenerates THIRD-PARTY-NOTICES.md from the shipped-binary dependency graph
# (the offline-protocol-uniffi crate — what actually links into the iOS/Android
# libraries and the Python wheel's native library) and copies it to every
# package that redistributes those binaries. Run after dependency changes.
#
# Requires: cargo install cargo-about --features cli --locked
set -euo pipefail
cd "$(dirname "$0")/.."

cargo about generate \
  --manifest-path crates/offline-protocol-uniffi/Cargo.toml \
  -o THIRD-PARTY-NOTICES.md \
  about.hbs

cp THIRD-PARTY-NOTICES.md bindings/react-native/THIRD-PARTY-NOTICES.md
cp THIRD-PARTY-NOTICES.md bindings/python/THIRD-PARTY-NOTICES.md

echo "Regenerated THIRD-PARTY-NOTICES.md (root, react-native, python)."
