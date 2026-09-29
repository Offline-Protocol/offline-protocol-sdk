#!/usr/bin/env bash

# Assemble the Swift package: a directory Swift Package Manager can build,
# made from the bridge sources where they already are.
#
# The sources are not moved and not copied into the repository. They have one
# home, bindings/react-native/ios, where the pod compiles them and where the
# Rust guards read them by path. This script copies them into an output
# directory beside a rendered manifest, and that directory is what ships: the
# release workflow pushes it to the distribution repository, and CI builds and
# tests the same tree on every pull request. See ADR 0025.
#
# The output directory appears whole or not at all. A refusal part way
# through leaves nothing behind for a later step to mistake for a package.
#
# Usage:
#   bash scripts/assemble-swift-package.sh \
#     --output <dir> --version <x.y.z> \
#     ( --xcframework-path <offline_protocolFFI.xcframework>
#     | --xcframework-url <https url> --xcframework-checksum <sha256> ) \
#     [--bridge-tests]
#
#   --xcframework-path   copy a local XCFramework into the package. For CI and
#                        for building the package on a developer's machine.
#   --xcframework-url    point the package at a published zip. For a release.
#   --bridge-tests       also copy the bridge's own suites out of ios/tests.
#                        They find their vectors by walking up to the
#                        repository root, so the output directory has to be
#                        inside the repository. Not for a release.
#
# Environment:
#   PACKAGE_SOURCE_ROOT  optional, defaults to the repository root
#   GITHUB_SHA           optional, recorded in the VERSION file

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

IOS="$ROOT/bindings/react-native/ios"
SWIFT="$ROOT/bindings/swift"

# The Swift module. Not `OfflineProtocol`: the generated bindings declare a
# class of that name, and a module that shares a name with one of its own
# types cannot be used to qualify anything in it.
MODULE="OfflineProtocolSDK"
# The clang module the generated Swift imports. Fixed by the generator.
FFI_MODULE="offline_protocolFFI"
# What the bridge's suites import, in the test harness they were written for.
HARNESS_MODULE="OfflineProtocol"

# The bridge sources that need React. Everything else at the top of ios/, and
# everything under the directories below, is part of the package. This is a
# list of what to leave out, not of what to take, so a new bridge source is in
# the package without being added anywhere, and one that imports React is
# refused below by name.
REACT_ONLY=(OfflineProtocolModule.swift)

# The directories of sources, the ones the podspec takes whole. A source in a
# new directory is in neither until it is named in both.
SOURCE_GROUPS=(ble mesh)

LEGAL_FILES=(LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md)

die() {
  echo "ERROR: $*" >&2
  exit 1
}

OUTPUT=""
VERSION=""
XCFRAMEWORK_PATH=""
XCFRAMEWORK_URL=""
XCFRAMEWORK_CHECKSUM=""
BRIDGE_TESTS=0

# value <flag> <remaining argument count> <next argument>
#
# A flag given last, or followed by another flag, has no value. Taken as one,
# `--output --bridge-tests` would assemble into a directory of that name.
value() {
  [ "$2" -ge 2 ] || die "$1 takes a value"
  case "$3" in
    --*) die "$1 takes a value, and '$3' is a flag" ;;
    "") die "$1 takes a value, and was given an empty one" ;;
  esac
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) value "$1" "$#" "${2:-}"; OUTPUT="$2"; shift 2 ;;
    --version) value "$1" "$#" "${2:-}"; VERSION="$2"; shift 2 ;;
    --xcframework-path) value "$1" "$#" "${2:-}"; XCFRAMEWORK_PATH="$2"; shift 2 ;;
    --xcframework-url) value "$1" "$#" "${2:-}"; XCFRAMEWORK_URL="$2"; shift 2 ;;
    --xcframework-checksum) value "$1" "$#" "${2:-}"; XCFRAMEWORK_CHECKSUM="$2"; shift 2 ;;
    --bridge-tests) BRIDGE_TESTS=1; shift ;;
    *) die "unknown argument: $1" ;;
  esac
done

[ -n "$OUTPUT" ] || die "--output is required"
[ -n "$VERSION" ] || die "--version is required"

# Every value below is matched whole, with [[ =~ ]] and not with grep. grep
# matches a line: a digest with a tool's warning on the line above it, which
# is what a workflow captures when the tool warns, has a line that matches.

# The version is written into the README a consumer copies from, and Swift
# Package Manager resolves nothing that is not a full semantic version.
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
[[ "$VERSION" =~ $SEMVER ]] ||
  die "--version must be a semantic version, without a leading v: '$VERSION'"

if [ -n "$XCFRAMEWORK_PATH" ]; then
  [ -z "$XCFRAMEWORK_URL$XCFRAMEWORK_CHECKSUM" ] ||
    die "--xcframework-path cannot be combined with --xcframework-url or --xcframework-checksum"
  [ -d "$XCFRAMEWORK_PATH" ] || die "no such XCFramework: $XCFRAMEWORK_PATH"
  [ "$(basename "$XCFRAMEWORK_PATH")" = "$FFI_MODULE.xcframework" ] ||
    die "the XCFramework must be called $FFI_MODULE.xcframework, the module the generated Swift imports"
else
  [ -n "$XCFRAMEWORK_URL" ] && [ -n "$XCFRAMEWORK_CHECKSUM" ] ||
    die "give either --xcframework-path, or both --xcframework-url and --xcframework-checksum"
  # Swift Package Manager accepts only https and only a zip, and says so when
  # a consumer resolves the package. The characters are limited to ones that
  # mean nothing inside a Swift string literal, which is where this goes.
  HTTPS_ZIP='^https://[A-Za-z0-9][A-Za-z0-9._~/%+-]*\.zip$'
  [[ "$XCFRAMEWORK_URL" =~ $HTTPS_ZIP ]] ||
    die "--xcframework-url must be an https url ending in .zip, of letters, digits and ._~/%+- only: '$XCFRAMEWORK_URL'"
  # A digest is pasted, templated and passed through a workflow before it
  # lands here. One that arrives empty, cut short or with something else
  # beside it would publish a package no consumer can resolve.
  SHA256='^[0-9a-f]{64}$'
  [[ "$XCFRAMEWORK_CHECKSUM" =~ $SHA256 ]] ||
    die "--xcframework-checksum must be 64 lowercase hex digits and nothing else: '$XCFRAMEWORK_CHECKSUM'"
  [ "$BRIDGE_TESTS" = 0 ] ||
    die "--bridge-tests is for building inside the repository, not for a release"
fi

# Never write into a directory that holds something: the output is what
# ships, and what was there would ship beside what is new.
if [ -e "$OUTPUT" ]; then
  [ -d "$OUTPUT" ] || die "--output exists and is not a directory: $OUTPUT"
  [ -z "$(ls -A "$OUTPUT")" ] || die "--output is not empty: $OUTPUT"
fi
mkdir -p "$(dirname "$OUTPUT")"
OUTPUT_PARENT="$(cd "$(dirname "$OUTPUT")" && pwd -P)"

# The bridge's suites walk up from their own path to the conformance vectors
# at the repository root. Assembled anywhere else they compile, and stop at
# the first test that wants a vector.
if [ "$BRIDGE_TESTS" = 1 ]; then
  ROOT_REAL="$(cd "$ROOT" && pwd -P)"
  case "$OUTPUT_PARENT/" in
    "$ROOT_REAL"/*) ;;
    *) die "--bridge-tests needs --output inside $ROOT_REAL, and $OUTPUT_PARENT is not" ;;
  esac
fi

[ -d "$IOS" ] || die "missing $IOS"
[ -f "$IOS/Generated/offline_protocol.swift" ] || die "missing the generated Swift bindings"
[ -f "$IOS/PrivacyInfo.xcprivacy" ] || die "missing $IOS/PrivacyInfo.xcprivacy"
[ -f "$SWIFT/Package.swift.template" ] || die "missing $SWIFT/Package.swift.template"
[ -f "$SWIFT/PACKAGE_README.md.template" ] || die "missing $SWIFT/PACKAGE_README.md.template"
for legal in "${LEGAL_FILES[@]}"; do
  [ -f "$ROOT/$legal" ] || die "missing $legal"
done

# Every excluded file has to exist. A name that matches nothing is a rename
# nobody carried over here, and the renamed file is then in the package.
for excluded in "${REACT_ONLY[@]}"; do
  [ -f "$IOS/$excluded" ] || die "REACT_ONLY names $excluded, which is not in $IOS"
done

# The oldest iOS the package admits is the pod's, and is not written again in
# the manifest. A manifest that promised an older system than the pod would
# promise it for a library nobody built for it.
IOS_DEPLOYMENT_TARGET="$(PACKAGE_SOURCE_ROOT="$ROOT" bash "$SCRIPT_DIR/ios-deployment-target.sh")"

# Everything is written beside the output and moved into place at the end.
STAGE="$(mktemp -d "$OUTPUT_PARENT/.assembling.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

is_react_only() {
  local name="$1" excluded
  for excluded in "${REACT_ONLY[@]}"; do
    [ "$name" = "$excluded" ] && return 0
  done
  return 1
}

SOURCES="$STAGE/Sources/$MODULE"
TESTS="$STAGE/Tests/${MODULE}Tests"
mkdir -p "$SOURCES/Generated" "$SOURCES/Bridge" "$TESTS"

cp "$IOS/Generated/offline_protocol.swift" "$SOURCES/Generated/"

# The top level of ios/, less the manifest of the test harness and the files
# that need React.
COPIED=0
for source in "$IOS"/*.swift; do
  name="$(basename "$source")"
  [ "$name" = "Package.swift" ] && continue
  is_react_only "$name" && continue
  cp "$source" "$SOURCES/Bridge/$name"
  COPIED=$((COPIED + 1))
done

for group in "${SOURCE_GROUPS[@]}"; do
  [ -d "$IOS/$group" ] || die "missing $IOS/$group"
  while IFS= read -r source; do
    relative="${source#"$IOS"/}"
    mkdir -p "$SOURCES/Bridge/$(dirname "$relative")"
    cp "$source" "$SOURCES/Bridge/$relative"
    COPIED=$((COPIED + 1))
  done < <(find "$IOS/$group" -name '*.swift' | sort)
done

[ "$COPIED" -gt 0 ] || die "copied no bridge sources from $IOS"

# Refuse a source that needs React, by name, here rather than as two hundred
# compiler errors on a Mac. The compiler is still the real check: it runs in
# CI on the tree this writes.
#
# Matched: `import React`, a submodule or a declaration of it, and the
# modules React Native names React_Something, behind any attribute or access
# modifier and an optional byte order mark. Not matched: a module that only
# begins with the letters, such as ReactiveSwift. An import inside
# `#if canImport(React)` is refused like any other. It would compile, and it
# is a source with a React half that belongs in OfflineProtocolModule.swift.
BOM=$'\xef\xbb\xbf'
REACT_IMPORT="^(${BOM})?[[:space:]]*((@[A-Za-z_]+(\\([^)]*\\))?|internal|private|fileprivate|public|package)[[:space:]]+)*import[[:space:]]+((class|struct|enum|protocol|func|var|let|typealias)[[:space:]]+)?React(_[A-Za-z0-9_]+)?([.;[:space:]]|\$)"
REACT_IMPORTS="$(LC_ALL=C grep -rlE "$REACT_IMPORT" "$SOURCES" || true)"
if [ -n "$REACT_IMPORTS" ]; then
  echo "ERROR: these sources import React and cannot be in the Swift package:" >&2
  while IFS= read -r found; do
    echo "  ${found#"$STAGE"/}" >&2
  done <<<"$REACT_IMPORTS"
  echo "Move the React half into OfflineProtocolModule.swift, or add the file to" >&2
  echo "REACT_ONLY in scripts/assemble-swift-package.sh if all of it needs React." >&2
  exit 1
fi

cp "$IOS/PrivacyInfo.xcprivacy" "$SOURCES/PrivacyInfo.xcprivacy"

# The package's own suites.
FOUND_TESTS=0
if [ -d "$SWIFT/Tests/${MODULE}Tests" ]; then
  for suite in "$SWIFT/Tests/${MODULE}Tests"/*.swift; do
    [ -f "$suite" ] || continue
    cp "$suite" "$TESTS/"
    FOUND_TESTS=$((FOUND_TESTS + 1))
  done
fi
[ "$FOUND_TESTS" -gt 0 ] || die "found no suites in $SWIFT/Tests/${MODULE}Tests"

# The bridge's suites, which import the harness module by name.
if [ "$BRIDGE_TESTS" = 1 ]; then
  mkdir -p "$TESTS/Bridge"
  for suite in "$IOS"/tests/*.swift; do
    name="$(basename "$suite")"
    sed "s/^@testable import $HARNESS_MODULE\$/@testable import $MODULE/" "$suite" >"$TESTS/Bridge/$name"
    # A suite that still imports the harness module would fail to compile
    # with "no such module", which names the symptom and not this line.
    grep -q "^@testable import $MODULE\$" "$TESTS/Bridge/$name" ||
      die "$name does not import $HARNESS_MODULE the way this script rewrites it"
  done
fi

if [ -n "$XCFRAMEWORK_PATH" ]; then
  cp -R "$XCFRAMEWORK_PATH" "$STAGE/$FFI_MODULE.xcframework"
  BINARY_TARGET=".binaryTarget(
            name: \"$FFI_MODULE\",
            path: \"$FFI_MODULE.xcframework\"
        )"
else
  BINARY_TARGET=".binaryTarget(
            name: \"$FFI_MODULE\",
            url: \"$XCFRAMEWORK_URL\",
            checksum: \"$XCFRAMEWORK_CHECKSUM\"
        )"
fi

# render <template> <output>
#
# With awk rather than sed: one value spans lines, and sed's replacement
# syntax gives `&` and the backslash a meaning. Each placeholder is rendered
# wherever it stands and must stand somewhere.
render() {
  BINARY_TARGET="$BINARY_TARGET" \
    IOS_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET" \
    PACKAGE_VERSION="$VERSION" \
    awk -v required="$3" '
    function put(placeholder, value,    at) {
      while ((at = index($0, placeholder)) > 0) {
        $0 = substr($0, 1, at - 1) value substr($0, at + length(placeholder))
        rendered[placeholder]++
      }
    }
    {
      put("@@BINARY_TARGET@@", ENVIRON["BINARY_TARGET"])
      put("@@IOS_DEPLOYMENT_TARGET@@", ENVIRON["IOS_DEPLOYMENT_TARGET"])
      put("@@VERSION@@", ENVIRON["PACKAGE_VERSION"])
      print
    }
    END {
      count = split(required, names, " ")
      for (i = 1; i <= count; i++) {
        if (rendered["@@" names[i] "@@"] < 1) exit 1
      }
    }
  ' "$1" >"$2" || die "$(basename "$1") must hold each of: $3"

  if grep -q '@@' "$2"; then
    die "the rendered $(basename "$2") still holds a placeholder"
  fi
}

render "$SWIFT/Package.swift.template" "$STAGE/Package.swift" "BINARY_TARGET IOS_DEPLOYMENT_TARGET"
render "$SWIFT/PACKAGE_README.md.template" "$STAGE/README.md" "VERSION IOS_DEPLOYMENT_TARGET"

for legal in "${LEGAL_FILES[@]}"; do
  cp "$ROOT/$legal" "$STAGE/$legal"
done

cat >"$STAGE/VERSION" <<VERSION_EOF
name=offline-protocol-swift
version=$VERSION
commit=${GITHUB_SHA:-unknown}
VERSION_EOF

# Into place, whole. The output was checked to be absent or empty above.
if [ -d "$OUTPUT" ]; then
  rmdir "$OUTPUT"
fi
mv "$STAGE" "$OUTPUT"
trap - EXIT

echo "Swift package assembled: $OUTPUT ($COPIED bridge sources, $FOUND_TESTS package suites)"
