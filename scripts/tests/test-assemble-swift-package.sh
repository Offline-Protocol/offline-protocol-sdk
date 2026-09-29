#!/usr/bin/env bash

# Drives scripts/assemble-swift-package.sh against a fixture tree.
#
# The script's output is meant to be a public package: a release is to push
# the tree it writes to the distribution repository and tag it, and neither a
# tag nor a resolved checksum can be taken back once it does. Nothing is
# pushed yet (ADR 0025). Building that tree needs a
# Mac and the Rust library, so the job that does runs the success path only.
# This runs everywhere, and its weight is on the other half: exactly what the
# script writes, and what it refuses.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ASSEMBLE="$REPO_ROOT/scripts/assemble-swift-package.sh"

FAILURES=0
ASSERTIONS=0
# A test that stops asserting passes. Raise this with every check added.
EXPECTED_ASSERTIONS=120

URL="https://example.com/releases/offline-protocol-1.2.3-swiftpm-xcframework.zip"
CHECKSUM="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
LOG="$WORK/log"

pass() {
  echo "  ok: $*"
  ASSERTIONS=$((ASSERTIONS + 1))
}

fail() {
  echo "  FAIL: $*" >&2
  ASSERTIONS=$((ASSERTIONS + 1))
  FAILURES=$((FAILURES + 1))
}

check() {
  local name="$1"
  shift
  if "$@"; then pass "$name"; else fail "$name"; fi
}

absent() { [ ! -e "$1" ]; }
holds() { grep -qF -- "$2" "$1"; }
lacks() { ! grep -qF -- "$2" "$1"; }
# The line is there, whole. `holds` would accept it with something after it.
holds_line() { grep -qxF -- "$2" "$1"; }

# Builds a valid source tree at $1: the real templates, fixture sources.
build_fixture() {
  local root="$1"
  local ios="$root/bindings/react-native/ios"
  local swift="$root/bindings/swift"

  mkdir -p "$ios/Generated" "$ios/ble/nested" "$ios/mesh" "$ios/tests" "$ios/libs" \
    "$swift/Tests/OfflineProtocolSDKTests"

  for legal in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md; do
    echo "fixture $legal" >"$root/$legal"
  done

  printf '  s.name = "MeshSdk"\n  s.platforms    = { :ios => "15.1" }\n' \
    >"$root/bindings/react-native/MeshSdk.podspec"

  echo "// generated" >"$ios/Generated/offline_protocol.swift"
  echo "fixture header" >"$ios/Generated/offline_protocolFFI.h"
  echo "fixture modulemap" >"$ios/Generated/offline_protocolFFI.modulemap"
  echo "fixture privacy" >"$ios/PrivacyInfo.xcprivacy"

  # A module that only begins with the letters is not React.
  printf 'import Foundation\nimport ReactiveSwift\nfinal class InternetManager {}\n' \
    >"$ios/InternetManager.swift"
  printf 'import Foundation\nimport React\nclass OfflineProtocolModule {}\n' \
    >"$ios/OfflineProtocolModule.swift"
  # What sits beside the sources and is not one.
  echo "// the harness manifest" >"$ios/Package.swift"
  echo "objective c" >"$ios/OfflineProtocolModule.m"
  echo "notes" >"$ios/BRIDGE_MAINTENANCE.md"
  echo "archive" >"$ios/libs/liboffline_protocol_uniffi.a"

  printf 'import CoreBluetooth\n' >"$ios/ble/Registry.swift"
  printf 'import Foundation\n' >"$ios/ble/nested/Deep.swift"
  printf 'import CryptoKit\n' >"$ios/mesh/Mesh.swift"
  printf 'import XCTest\n@testable import OfflineProtocol\n' >"$ios/tests/MeshTests.swift"

  cp "$REPO_ROOT/bindings/swift/Package.swift.template" "$swift/"
  cp "$REPO_ROOT/bindings/swift/PACKAGE_README.md.template" "$swift/"
  printf 'import XCTest\n@testable import OfflineProtocolSDK\n' \
    >"$swift/Tests/OfflineProtocolSDKTests/LinkTests.swift"
}

# assemble <root> <args>...   Output goes to $LOG.
assemble() {
  local root="$1"
  shift
  PACKAGE_SOURCE_ROOT="$root" GITHUB_SHA="abc123" bash "$ASSEMBLE" "$@" >"$LOG" 2>&1
}

# tree <dir>   Every file under it, sorted, relative.
tree() { (cd "$1" && find . -type f | sed 's|^\./||' | LC_ALL=C sort); }

# beside <output>   What is in the directory the output goes into, less the
# output itself. The script stages there, under a name that begins with a dot.
beside() {
  local parent
  parent="$(dirname "$1")"
  [ -d "$parent" ] || return 0
  find "$parent" -mindepth 1 -maxdepth 1 ! -name "$(basename "$1")"
}

# refuses <name> <expected text> <output that must not exist> <root> <args>...
#
# The script must fail, say why in words that name the cause, and leave
# nothing where the package would have been.
refuses() {
  local name="$1" expected="$2" output="$3"
  shift 3
  if assemble "$@"; then
    fail "$name: it succeeded"
    fail "$name: it said nothing, having succeeded"
    fail "$name: it wrote a package"
    return
  fi
  pass "$name: it fails"
  if grep -qF -- "$expected" "$LOG"; then
    pass "$name: it says why"
  else
    fail "$name: the message does not hold '$expected': $(tr '\n' ' ' <"$LOG")"
  fi
  if [ -z "$output" ]; then
    pass "$name: the output was there before, and is checked by the caller"
  elif [ -e "$output" ]; then
    fail "$name: it left $(tree "$output" | tr '\n' ' ')"
  elif [ -n "$(beside "$output")" ]; then
    fail "$name: it left beside the output: $(beside "$output" | tr '\n' ' ')"
  else
    pass "$name: it leaves nothing behind"
  fi
}

release() {
  assemble "$1" --output "$2" --version 1.2.3 \
    --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
}

echo "a release"
ROOT="$WORK/release"
OUT="$WORK/release-out/offline-protocol-swift"
build_fixture "$ROOT"
if release "$ROOT" "$OUT"; then
  pass "it assembles"
else
  fail "it does not assemble: $(tr '\n' ' ' <"$LOG")"
fi
# Every file, and no other. What is not on this list is not in the package:
# the React module, the harness manifest, the Objective-C shim, the header,
# the archive, the bridge's suites, a binary.
EXPECTED_RELEASE="EXPORT.md
LICENSE
LICENSE-COMMERCIAL.md
Package.swift
README.md
Sources/OfflineProtocolSDK/Bridge/InternetManager.swift
Sources/OfflineProtocolSDK/Bridge/ble/Registry.swift
Sources/OfflineProtocolSDK/Bridge/ble/nested/Deep.swift
Sources/OfflineProtocolSDK/Bridge/mesh/Mesh.swift
Sources/OfflineProtocolSDK/Generated/offline_protocol.swift
Sources/OfflineProtocolSDK/PrivacyInfo.xcprivacy
THIRD-PARTY-NOTICES.md
Tests/OfflineProtocolSDKTests/LinkTests.swift
VERSION"
if [ "$(tree "$OUT")" = "$EXPECTED_RELEASE" ]; then
  pass "it holds exactly the package"
else
  fail "it holds: $(tree "$OUT" | tr '\n' ' ')"
fi
check "nothing is a link" test -z "$(find "$OUT" -type l)"
check "the manifest names the url" holds_line "$OUT/Package.swift" "            url: \"$URL\","
check "the manifest names the checksum" holds_line "$OUT/Package.swift" "            checksum: \"$CHECKSUM\""
check "the manifest takes the pod's deployment target" holds "$OUT/Package.swift" '.iOS("15.1")'
check "the manifest holds no placeholder" lacks "$OUT/Package.swift" "@@"
check "the README names the version" holds "$OUT/README.md" 'from: "1.2.3"'
check "the README takes the pod's deployment target" holds "$OUT/README.md" "iOS 15.1 or later"
check "the README holds no placeholder" lacks "$OUT/README.md" "@@"
check "the version file names the package" holds_line "$OUT/VERSION" "name=offline-protocol-swift"
check "the version file names the version" holds_line "$OUT/VERSION" "version=1.2.3"
check "the version file names the commit" holds_line "$OUT/VERSION" "commit=abc123"
check "no staging directory is left" test -z "$(beside "$OUT")"

echo "a release into a directory that is there and empty"
mkdir -p "$WORK/empty-out"
if release "$ROOT" "$WORK/empty-out"; then
  pass "it assembles"
else
  fail "it does not assemble: $(tr '\n' ' ' <"$LOG")"
fi
check "it holds the manifest" test -f "$WORK/empty-out/Package.swift"

echo "a local build"
# Inside the fixture root, where the bridge's suites have to be.
OUT="$ROOT/build/offline-protocol-swift"
mkdir -p "$WORK/offline_protocolFFI.xcframework/ios-arm64"
echo "fixture plist" >"$WORK/offline_protocolFFI.xcframework/Info.plist"
if assemble "$ROOT" --output "$OUT" --version 1.2.3-rc.1 \
  --xcframework-path "$WORK/offline_protocolFFI.xcframework" --bridge-tests; then
  pass "it assembles"
else
  fail "it does not assemble: $(tr '\n' ' ' <"$LOG")"
fi
EXPECTED_LOCAL="EXPORT.md
LICENSE
LICENSE-COMMERCIAL.md
Package.swift
README.md
Sources/OfflineProtocolSDK/Bridge/InternetManager.swift
Sources/OfflineProtocolSDK/Bridge/ble/Registry.swift
Sources/OfflineProtocolSDK/Bridge/ble/nested/Deep.swift
Sources/OfflineProtocolSDK/Bridge/mesh/Mesh.swift
Sources/OfflineProtocolSDK/Generated/offline_protocol.swift
Sources/OfflineProtocolSDK/PrivacyInfo.xcprivacy
THIRD-PARTY-NOTICES.md
Tests/OfflineProtocolSDKTests/Bridge/MeshTests.swift
Tests/OfflineProtocolSDKTests/LinkTests.swift
VERSION
offline_protocolFFI.xcframework/Info.plist"
if [ "$(tree "$OUT")" = "$EXPECTED_LOCAL" ]; then
  pass "it holds exactly the package, the binary and the bridge's suites"
else
  fail "it holds: $(tree "$OUT" | tr '\n' ' ')"
fi
check "the binary is a copy, not a link" test -z "$(find "$OUT" -type l)"
check "the manifest names its path" holds_line "$OUT/Package.swift" '            path: "offline_protocolFFI.xcframework"'
check "the manifest names no url" lacks "$OUT/Package.swift" "url:"
BRIDGE_SUITE="$OUT/Tests/OfflineProtocolSDKTests/Bridge/MeshTests.swift"
check "a bridge suite imports the package" holds_line "$BRIDGE_SUITE" "@testable import OfflineProtocolSDK"
check "a pre-release version is a version" holds "$OUT/README.md" 'from: "1.2.3-rc.1"'

echo "what it refuses: sources"

# The failure the exclusion list exists for: a shared source grows a React
# import, and the package stops building for every consumer.
react_import() {
  local name="$1" line="$2" file="$3"
  local root="$WORK/react-$ASSERTIONS"
  build_fixture "$root"
  printf 'import Foundation\n%s\n' "$line" >"$root/bindings/react-native/ios/$file"
  refuses "$name" "Bridge/$file" "$root-out/p" "$root" --output "$root-out/p" --version 1.2.3 \
    --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
}
react_import "a shared source that imports React" "import React" InternetManager.swift
react_import "an import under a directory" "import React" ble/nested/Deep.swift
react_import "an indented import" "    import React" InternetManager.swift
react_import "an import behind an attribute" "@_implementationOnly import React" InternetManager.swift
react_import "an import behind an access modifier" "internal import React" InternetManager.swift
react_import "an import of part of React" "import class React.RCTBridge" InternetManager.swift
react_import "an import of one of React's modules" "import React_Core" InternetManager.swift

# A rename nobody carried over: the list matches nothing, so the renamed
# file, which still needs React, would be taken.
ROOT="$WORK/renamed"
build_fixture "$ROOT"
mv "$ROOT/bindings/react-native/ios/OfflineProtocolModule.swift" \
  "$ROOT/bindings/react-native/ios/OfflineProtocolBridge.swift"
refuses "an exclusion that matches no file" "REACT_ONLY names OfflineProtocolModule.swift" \
  "$WORK/renamed-out/p" "$ROOT" --output "$WORK/renamed-out/p" --version 1.2.3 \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"

# The library is built for the pod's target. Without the number the manifest
# would have to invent one.
ROOT="$WORK/nopod"
build_fixture "$ROOT"
printf '  s.name = "MeshSdk"\n' >"$ROOT/bindings/react-native/MeshSdk.podspec"
refuses "a podspec that names no deployment target" "and found 0 such lines" \
  "$WORK/nopod-out/p" "$ROOT" --output "$WORK/nopod-out/p" --version 1.2.3 \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"

# Two declarations, and no way to say which one the pod means.
ROOT="$WORK/twopods"
build_fixture "$ROOT"
printf '  s.platforms = { :ios => "13.0" }\n  s.platforms = { :ios => "15.1" }\n' \
  >"$ROOT/bindings/react-native/MeshSdk.podspec"
refuses "a podspec that names two deployment targets" "and found 2 such lines" \
  "$WORK/twopods-out/p" "$ROOT" --output "$WORK/twopods-out/p" --version 1.2.3 \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"

# A bridge suite written against something other than the harness module.
ROOT="$WORK/suite"
build_fixture "$ROOT"
printf 'import XCTest\nimport OfflineProtocol\n' >"$ROOT/bindings/react-native/ios/tests/MeshTests.swift"
refuses "a bridge suite that imports another way" "MeshTests.swift does not import OfflineProtocol" \
  "$ROOT/build/p" "$ROOT" --output "$ROOT/build/p" --version 1.2.3 \
  --xcframework-path "$WORK/offline_protocolFFI.xcframework" --bridge-tests

echo "what it refuses: arguments"

ROOT="$WORK/valid"
build_fixture "$ROOT"
N=0
# argument <name> <expected text> <args>...   Each into an output of its own.
argument() {
  local name="$1" expected="$2"
  shift 2
  N=$((N + 1))
  refuses "$name" "$expected" "$WORK/arg-$N/p" "$ROOT" --output "$WORK/arg-$N/p" "$@"
}

argument "a checksum cut short" "64 lowercase hex digits" \
  --version 1.2.3 --xcframework-url "$URL" --xcframework-checksum "0123456789abcdef"
argument "a checksum one digit too long" "64 lowercase hex digits" \
  --version 1.2.3 --xcframework-url "$URL" --xcframework-checksum "${CHECKSUM}0"
argument "a checksum in capitals" "64 lowercase hex digits" \
  --version 1.2.3 --xcframework-url "$URL" \
  --xcframework-checksum "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
# What a workflow captures when the tool that prints the digest also warns.
argument "a checksum with a line above it" "64 lowercase hex digits" \
  --version 1.2.3 --xcframework-url "$URL" \
  --xcframework-checksum "warning: something
$CHECKSUM"
argument "a url that is not https" "https url ending in .zip" \
  --version 1.2.3 --xcframework-url "http://example.com/a.zip" --xcframework-checksum "$CHECKSUM"
argument "a url that is not a zip" "https url ending in .zip" \
  --version 1.2.3 --xcframework-url "https://example.com/a.tar.gz" --xcframework-checksum "$CHECKSUM"
argument "a url that would end the string it goes in" "https url ending in .zip" \
  --version 1.2.3 --xcframework-url 'https://example.com/a".zip' --xcframework-checksum "$CHECKSUM"
argument "a version with a leading v" "semantic version" \
  --version v1.2.3 --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
argument "a version of two parts" "semantic version" \
  --version 1.2 --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
argument "a version with a line above it" "semantic version" \
  --version "note
1.2.3" --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
argument "the bridge's suites in a release" "not for a release" \
  --version 1.2.3 --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM" --bridge-tests
argument "a binary and a url" "cannot be combined" \
  --version 1.2.3 --xcframework-path "$WORK/offline_protocolFFI.xcframework" \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
argument "a binary that is not there" "no such XCFramework" \
  --version 1.2.3 --xcframework-path "$WORK/missing/offline_protocolFFI.xcframework"
mkdir -p "$WORK/misnamed.xcframework"
argument "a binary under another name" "must be called offline_protocolFFI.xcframework" \
  --version 1.2.3 --xcframework-path "$WORK/misnamed.xcframework"
argument "an argument it does not know" "unknown argument: --force" \
  --version 1.2.3 --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM" --force
argument "a flag given a flag" "--version takes a value" \
  --version --bridge-tests
argument "a flag given nothing" "--xcframework-checksum takes a value" \
  --version 1.2.3 --xcframework-url "$URL" --xcframework-checksum
# Outside the repository the bridge's suites compile and cannot find a vector.
argument "the bridge's suites outside the repository" "needs --output inside" \
  --version 1.2.3 --xcframework-path "$WORK/offline_protocolFFI.xcframework" --bridge-tests

echo "what it refuses: the output"

mkdir -p "$WORK/occupied"
echo "left over" >"$WORK/occupied/stale.swift"
refuses "an output directory that holds something" "is not empty" "" \
  "$ROOT" --output "$WORK/occupied" --version 1.2.3 \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"
check "and what it held is untouched" test "$(tree "$WORK/occupied")" = "stale.swift"

# A clone of the distribution repository, or a directory xcodebuild ran in.
mkdir -p "$WORK/hidden/.git"
refuses "an output directory that holds only a hidden one" "is not empty" "" \
  "$ROOT" --output "$WORK/hidden" --version 1.2.3 \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"

echo "file" >"$WORK/a-file"
refuses "an output that is a file" "is not a directory" "" \
  "$ROOT" --output "$WORK/a-file" --version 1.2.3 \
  --xcframework-url "$URL" --xcframework-checksum "$CHECKSUM"

if [ "$ASSERTIONS" != "$EXPECTED_ASSERTIONS" ]; then
  echo "  FAIL: ran $ASSERTIONS assertions, expected $EXPECTED_ASSERTIONS" >&2
  FAILURES=$((FAILURES + 1))
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES failure(s)" >&2
  exit 1
fi

echo "all $ASSERTIONS assertions passed"
