#!/usr/bin/env bash

# Pins scripts/pep440-version.sh: every tag shape a release uses, and the
# shapes it refuses. A wrong answer here is a wheel numbered as the final
# release, which PyPI then never lets that release have.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONVERT="$SCRIPT_DIR/../pep440-version.sh"
FAILURES=0

expect() {
  local input="$1" want="$2" got
  got="$(bash "$CONVERT" "$input" 2>/dev/null || echo REFUSED)"
  if [ "$got" = "$want" ]; then
    echo "  ok - $input -> $want"
  else
    echo "  FAIL - $input -> $got, expected $want" >&2
    FAILURES=$((FAILURES + 1))
  fi
}

expect 0.28.0 0.28.0
expect 10.0.12 10.0.12
expect 0.28.0-rc.1 0.28.0rc1
expect 0.28.0-rc1 0.28.0rc1
expect 0.28.0-rc.12 0.28.0rc12
expect 0.28.0-alpha.2 0.28.0a2
expect 0.28.0-beta.3 0.28.0b3
expect 0.0.0-dev 0.0.0.dev0
expect 1.2.3-dev.4 1.2.3.dev4
# Refused: no agreed spelling, a local version PyPI rejects, not a version.
expect 0.28.0-rc REFUSED
expect 0.28.0-test REFUSED
expect 0.28.0-rc.1.2 REFUSED
expect 0.28.0+build.5 REFUSED
expect 0.28 REFUSED
expect v0.28.0 REFUSED
expect "" REFUSED

if [ "$FAILURES" -eq 0 ]; then
  echo "All version conversions passed."
else
  exit 1
fi
