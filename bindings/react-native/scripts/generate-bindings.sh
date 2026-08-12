#!/usr/bin/env bash

# Regenerate the UniFFI bindings after a UDL change (`npm run generate:bindings`).
#
# Kept as a wrapper because the npm script and CI both call this path, but the
# work — and every language, not just Swift and Kotlin — lives in the repo-root
# scripts/generate-bindings.sh. Swift and Kotlin are deliberately not
# regenerable on their own: see the header there for why a partial set fails at
# runtime rather than at build time.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

exec bash "$PROJECT_ROOT/scripts/generate-bindings.sh" "$@"
