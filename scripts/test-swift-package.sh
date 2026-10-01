#!/usr/bin/env bash

# Test the assembled Swift package on an iOS simulator: its own suites, then
# an application that depends on it.
#
# Expects the package at build/offline-protocol-swift, where
# scripts/assemble-swift-package.sh writes it and where the consumer check's
# manifest looks for it. Writes the two logs and the derived data under
# build/. Run by the Swift Package job on every pull request, and by the
# release on the package built from the release's own libraries, which is the
# one the release then publishes. One script, so the two cannot drift apart.
#
# Usage:
#   bash scripts/test-swift-package.sh
#
# Needs xcodebuild and an available iOS simulator (scripts/pick-ios-simulator.sh).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD="$ROOT/build"
PACKAGE="$BUILD/offline-protocol-swift"
CONSUMER="$ROOT/bindings/swift/consumer-check"

[ -f "$PACKAGE/Package.swift" ] || {
  echo "ERROR: no assembled package at $PACKAGE" >&2
  exit 1
}

SIMULATOR="$(bash "$SCRIPT_DIR/pick-ios-simulator.sh")"

# xcodebuild test, with the log kept whole on disk and only what failed, and
# the totals, printed. The log is thousands of lines.
run_tests() {
  local directory="$1" scheme="$2" derived="$3" log="$4" tail_lines="$5"
  local status=0
  (cd "$directory" && xcodebuild test -scheme "$scheme" \
    -destination "platform=iOS Simulator,id=$SIMULATOR" \
    -derivedDataPath "$derived" >"$log" 2>&1) || status=$?
  grep -E "error:|Test Case .* failed|Executed [0-9]+ tests?|\*\* TEST" "$log" |
    tail -"$tail_lines" || true
  if [ "$status" != 0 ]; then
    echo "::error::xcodebuild test exited $status ($scheme)"
    exit "$status"
  fi
}

run_tests "$PACKAGE" OfflineProtocolSDK "$BUILD/package-build" "$BUILD/package-test.log" 60

# A run that built and executed nothing also exits 0. Every suite file holds
# one suite, named after the file.
MISSING=0
while IFS= read -r suite; do
  name="$(basename "$suite" .swift)"
  grep -q "Test Suite '$name' passed" "$BUILD/package-test.log" || {
    echo "::error::$name did not run"
    MISSING=$((MISSING + 1))
  }
done < <(find "$PACKAGE/Tests" -name '*.swift' | sort)
[ "$MISSING" = 0 ]

# The package from an application's side: the product by the name the README
# gives, a plain import, and a call into the library from a module that is not
# the package.
run_tests "$CONSUMER" ConsumerCheck-Package "$BUILD/consumer-build" "$BUILD/consumer-test.log" 30
grep -q "Test Suite 'UseTests' passed" "$BUILD/consumer-test.log" || {
  echo "::error::UseTests did not run"
  exit 1
}
