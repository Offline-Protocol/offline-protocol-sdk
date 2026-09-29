#!/usr/bin/env bash

# Build the XCFramework the Swift package links: the same static archives the
# pod ships, with the FFI header and its module map inside each slice.
#
# The pod's XCFramework carries no headers. CocoaPods finds the header through
# the podspec's search paths, which Swift Package Manager does not have: a
# binary target is imported through whatever its slices carry, and nothing
# else. So the package needs an XCFramework of its own, and this builds it.
#
# WHY THE HEADERS SIT ONE DIRECTORY DOWN
#
# Xcode copies the Headers directory of every binary target an application
# links into one shared include directory. A module map has to be called
# `module.modulemap` to be found, so two libraries that each ship
# `Headers/module.modulemap` overwrite one another there, and the build stops
# with "multiple commands produce". Every library UniFFI generates ships that
# file, so an application that links this SDK beside any other Rust library
# would not build. Clang also looks for a module map in a directory named
# after the module, and that is where this one goes.
#
# Usage:
#   bash scripts/package-swiftpm-xcframework.sh <output dir> <archive>...
#
# Each archive becomes one slice: pass the device archive and the fat simulator
# archive out of the pod's XCFramework for a release, or one simulator archive
# to run the package's tests. Two archives for one platform do not make two
# slices: xcodebuild refuses two architectures of it and quietly writes one
# slice for the same archive given twice, and this script refuses the second.
# So the two simulator architectures have to arrive as one fat archive. Needs
# xcodebuild.
#
# Environment:
#   PACKAGE_SOURCE_ROOT  optional, defaults to the repository root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

# The clang module the generated Swift imports. The XCFramework, the binary
# target in the package manifest and the directory under Headers all take
# this name, and the last of the three is the one clang matches on.
FFI_MODULE="offline_protocolFFI"
GENERATED="$ROOT/bindings/react-native/ios/Generated"

die() {
  echo "ERROR: $*" >&2
  exit 1
}

[ "$#" -ge 2 ] || die "usage: $0 <output dir> <archive>..."

OUTPUT_DIR="$1"
shift

[ -f "$GENERATED/$FFI_MODULE.h" ] || die "missing $GENERATED/$FFI_MODULE.h"
[ -f "$GENERATED/$FFI_MODULE.modulemap" ] || die "missing $GENERATED/$FFI_MODULE.modulemap"
command -v xcodebuild >/dev/null || die "xcodebuild is not on PATH: an XCFramework can only be built on a Mac"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/Headers/$FFI_MODULE"
cp "$GENERATED/$FFI_MODULE.h" "$STAGE/Headers/$FFI_MODULE/$FFI_MODULE.h"
cp "$GENERATED/$FFI_MODULE.modulemap" "$STAGE/Headers/$FFI_MODULE/module.modulemap"

ARGS=()
for archive in "$@"; do
  [ -f "$archive" ] || die "no such archive: $archive"
  ARGS+=(-library "$archive" -headers "$STAGE/Headers")
done

XCFRAMEWORK="$OUTPUT_DIR/$FFI_MODULE.xcframework"
mkdir -p "$OUTPUT_DIR"
# -create-xcframework refuses to write over an existing bundle.
rm -rf "$XCFRAMEWORK"
xcodebuild -create-xcframework "${ARGS[@]}" -output "$XCFRAMEWORK"

# The package imports the module through exactly this layout, so look at what
# was written rather than trust the exit code. A bundle that fails is removed:
# left in place it is a directory of the right name for the next step to use.
refuse() {
  rm -rf "$XCFRAMEWORK"
  die "$*"
}

SLICES=0
for slice in "$XCFRAMEWORK"/*/; do
  SLICES=$((SLICES + 1))
  [ -f "$slice/Headers/$FFI_MODULE/module.modulemap" ] ||
    refuse "slice $(basename "$slice") has no Headers/$FFI_MODULE/module.modulemap"
  [ -f "$slice/Headers/$FFI_MODULE/$FFI_MODULE.h" ] ||
    refuse "slice $(basename "$slice") has no Headers/$FFI_MODULE/$FFI_MODULE.h"
done
[ "$SLICES" = "$#" ] || refuse "asked for $# slices, the XCFramework holds $SLICES"

echo "Swift package XCFramework created: $XCFRAMEWORK ($SLICES slices)"
