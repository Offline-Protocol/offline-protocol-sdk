#!/usr/bin/env bash

# Print the oldest iOS the SDK supports: the pod's deployment target, read
# out of the podspec.
#
# The podspec is where the number lives. Whatever else needs it, the Swift
# package's manifest and the build of the library inside it, asks here and
# does not write it down again: a second copy is a number somebody raises in
# one place, and a package that promises a system its library was not built
# for.
#
# Usage:
#   bash scripts/ios-deployment-target.sh
#
# Environment:
#   PACKAGE_SOURCE_ROOT  optional, defaults to the repository root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
PODSPEC="$ROOT/bindings/react-native/MeshSdk.podspec"

[ -f "$PODSPEC" ] || {
  echo "ERROR: missing $PODSPEC" >&2
  exit 1
}

# One line, of one shape. Two would be a question with no answer here, and
# the first of them is not an answer.
FOUND="$(sed -n 's/^[[:space:]]*s\.platforms[[:space:]]*=.*:ios[[:space:]]*=>[[:space:]]*"\([0-9][0-9]*\.[0-9][0-9.]*\)".*/\1/p' "$PODSPEC")"
COUNT="$(printf '%s' "$FOUND" | grep -c . || true)"

if [ "$COUNT" != 1 ]; then
  echo "ERROR: expected $PODSPEC to declare s.platforms = { :ios => \"<version>\" } once, and found $COUNT such lines" >&2
  exit 1
fi

echo "$FOUND"
