#!/usr/bin/env bash

# `npm run build:ios` — kept as the documented entry point, but the work lives
# in build-uniffi-ios.sh.
#
# This used to be a near-copy of that script minus the binding generation, and
# that made it the largest remaining hole in the single-entry-point rule: it
# produced a fresh xcframework beside whatever Swift happened to be committed,
# which is the ABI mismatch scripts/generate-bindings.sh exists to prevent —
# not on a missing-bindgen edge case, but on every run. Six example READMEs
# route new contributors through `npm run build:all`, so this was also the most
# travelled path in the repo.
#
# The copy had drifted in the SDK's favour too: build-uniffi-ios.sh handles
# cargo placing the staticlib in release/deps/ as well as release/, which this
# script did not. So it is a strict superset, and delegating both closes the
# hole and deletes a second copy of the packaging logic.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec bash "$SCRIPT_DIR/build-uniffi-ios.sh" "$@"
