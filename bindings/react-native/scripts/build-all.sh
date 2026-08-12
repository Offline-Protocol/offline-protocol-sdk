#!/usr/bin/env bash

# `npm run build:all` — kept as the documented entry point (six example READMEs
# route new contributors through it), but the work lives in build-uniffi-all.sh.
#
# This used to orchestrate build-ios.sh and build-android.sh, the two copies
# that built native libraries without regenerating bindings. See those files
# for why that was the single most travelled way to produce a mismatched
# artifact set.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec bash "$SCRIPT_DIR/build-uniffi-all.sh" "$@"
