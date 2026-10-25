#!/usr/bin/env bash
#
# Copies the UniFFI mobile libraries from the React Native tree into the
# Flutter plugin's staged locations, which its Gradle build and podspec prefer
# over the React Native ones when they hold the library (docs/bridges/flutter.md,
# F8b). Both locations are gitignored, so nothing staged here is committed or
# reaches a published package.
#
# The source is bindings/react-native after npm run build:uniffi:all.
#
# Usage:
#   ./bindings/flutter/scripts/stage-plugin-native-artifacts.sh          # write
#   ./bindings/flutter/scripts/stage-plugin-native-artifacts.sh --check  # verify

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN="$REPO_ROOT/bindings/flutter/offline_protocol"
RN_ANDROID="$REPO_ROOT/bindings/react-native/android/src/main/jniLibs"
RN_IOS="$REPO_ROOT/bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework"
DEST_ANDROID="$PLUGIN/android/src/main/jniLibs"
DEST_IOS="$PLUGIN/ios/Frameworks/offline_protocol_uniffi.xcframework"

CHECK=false
if [[ "${1:-}" == "--check" ]]; then
  CHECK=true
fi

require_source() {
  if [[ ! -d "$RN_ANDROID" ]] || ! compgen -G "$RN_ANDROID/*/libuniffi_offline_protocol.so" >/dev/null; then
    echo "Missing Android jniLibs under $RN_ANDROID (run npm run build:uniffi:android in bindings/react-native)" >&2
    exit 1
  fi
  if [[ ! -d "$RN_IOS/ios-arm64" ]]; then
    echo "Missing iOS XCFramework at $RN_IOS (run npm run build:uniffi:ios in bindings/react-native)" >&2
    exit 1
  fi
}

sync_tree() {
  local src="$1"
  local dest="$2"
  mkdir -p "$(dirname "$dest")"
  rm -rf "$dest"
  mkdir -p "$dest"
  rsync -a "$src/" "$dest/"
}

if $CHECK; then
  require_source
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    if [[ ! -f "$DEST_ANDROID/$abi/libuniffi_offline_protocol.so" ]]; then
      echo "Staged Android library missing: $DEST_ANDROID/$abi/libuniffi_offline_protocol.so" >&2
      exit 1
    fi
  done
  if [[ ! -f "$DEST_IOS/ios-arm64/liboffline_protocol_uniffi.a" ]]; then
    echo "Staged iOS XCFramework missing at $DEST_IOS" >&2
    exit 1
  fi
  echo "Flutter plugin native artifacts are staged."
  exit 0
fi

require_source
sync_tree "$RN_ANDROID" "$DEST_ANDROID"
sync_tree "$RN_IOS" "$DEST_IOS"
echo "Staged jniLibs -> $DEST_ANDROID"
echo "Staged XCFramework -> $DEST_IOS"
