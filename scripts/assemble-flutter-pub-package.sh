#!/usr/bin/env bash
#
# Assemble a self-contained Flutter plugin tree for pub.dev.
# The in-repository plugin reaches shared bridge sources under
# bindings/react-native; a pub consumer has no such tree. This script copies
# the bridge, stages prebuilt UniFFI libraries, and writes the version pin the
# Gradle build reads, then verifies the layout a `flutter pub publish` tarball
# must carry.
#
# Run after npm run build:uniffi:all (or after a release has downloaded the
# same artifacts into the React Native tree) and
# bindings/flutter/scripts/stage-plugin-native-artifacts.sh.
#
# Usage:
#   bash scripts/assemble-flutter-pub-package.sh \
#     --output build/flutter-pub-package --version 0.33.0
#
# Environment:
#   PACKAGE_SOURCE_ROOT  optional — defaults to the repository root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$REPO_ROOT}"
PLUGIN="$ROOT/bindings/flutter/offline_protocol"
STAGE="$REPO_ROOT/bindings/flutter/scripts/stage-plugin-native-artifacts.sh"

OUTPUT=""
VERSION=""

die() {
  echo "ERROR: $*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      [ "$#" -ge 2 ] || die "--output takes a path"
      OUTPUT="$2"
      shift 2
      ;;
    --version)
      [ "$#" -ge 2 ] || die "--version takes a value"
      VERSION="$2"
      shift 2
      ;;
    *)
      die "unknown argument: $1 (expected --output and --version)"
      ;;
  esac
done

[ -n "$OUTPUT" ] || die "--output is required"
[ -n "$VERSION" ] || die "--version is required"

# pub forbids leading v; release tags carry it.
VERSION="${VERSION#v}"

if [ -e "$OUTPUT" ]; then
  if [ -n "$(find "$OUTPUT" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    die "$OUTPUT exists and is not empty"
  fi
fi

stage_and_verify() {
  bash "$STAGE"
  bash "$STAGE" --check
}

assemble() {
  mkdir -p "$OUTPUT"

  rsync -a \
    --exclude '.dart_tool/' \
    --exclude 'example/' \
    --exclude 'pubspec.lock' \
    "$PLUGIN/" "$OUTPUT/"

  mkdir -p "$OUTPUT/android/bridge-rn"
  rsync -a \
    --exclude 'build/' \
    --exclude '.gradle/' \
    "$ROOT/bindings/react-native/android/" "$OUTPUT/android/bridge-rn/"

  printf '{"version":"%s"}\n' "$VERSION" >"$OUTPUT/android/pub_bridge_version.json"

  IOS_DIR="$OUTPUT/ios"
  RN_IOS="$ROOT/bindings/react-native/ios"
  DEST="$IOS_DIR/BridgeRN"
  rm -rf "$DEST"
  mkdir -p "$DEST"
  rsync -a \
    --exclude OfflineProtocolModule.swift \
    --exclude OfflineProtocolModule.m \
    --exclude ProtocolEngineHostBridgeReactNative.swift \
    --exclude tests/ \
    --exclude Package.swift \
    "$RN_IOS/" "$DEST/"

  for legal in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md; do
  cp "$REPO_ROOT/$legal" "$OUTPUT/"
  done

  cat >"$OUTPUT/.pubignore" <<'PUBIGNORE'
# Maintainer paths only; the publish tree is not a git checkout.
.dart_tool/
.pub/
example/
PUBIGNORE

  grep -q "^version: " "$OUTPUT/pubspec.yaml" || die "pubspec.yaml has no version"
  perl -i -pe "s/^version: .*/version: $VERSION/" "$OUTPUT/pubspec.yaml"
  if ! grep -q '^publish_to:' "$OUTPUT/pubspec.yaml"; then
    perl -i -0pe 's/^environment:/publish_to: https:\/\/pub.dev\n\nenvironment:/m' "$OUTPUT/pubspec.yaml"
  fi

  verify_layout "$OUTPUT"
}

verify_layout() {
  local root="$1"
  local fail=0
  check() {
    if [ -e "$root/$1" ]; then
      :
    else
      echo "missing: $1" >&2
      fail=1
    fi
  }
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    check "android/src/main/jniLibs/$abi/libuniffi_offline_protocol.so"
  done
  check "ios/Frameworks/offline_protocol_uniffi.xcframework/ios-arm64/liboffline_protocol_uniffi.a"
  check "ios/BridgeRN/Generated/offline_protocol.swift"
  check "android/bridge-rn/src/main/java/uniffi/offline_protocol/offline_protocol.kt"
  check "android/pub_bridge_version.json"
  check "pubspec.yaml"
  [ "$fail" -eq 0 ] || die "assembled Flutter package failed layout checks"
}

stage_and_verify
assemble
echo "Assembled Flutter pub package at $OUTPUT (version $VERSION)"
