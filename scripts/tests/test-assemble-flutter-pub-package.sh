#!/usr/bin/env bash
# Exercises scripts/assemble-flutter-pub-package.sh on a fixture tree (refusals
# and one success path). The success path needs no NDK: it stubs jniLibs,
# XCFramework, and generated bindings where the layout check looks.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ASSEMBLE="$REPO_ROOT/scripts/assemble-flutter-pub-package.sh"
STAGE="$REPO_ROOT/bindings/flutter/scripts/stage-plugin-native-artifacts.sh"

FAILURES=0
pass() { echo "  ok: $*"; }
fail() { echo "  FAIL: $*" >&2; FAILURES=$((FAILURES + 1)); }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

build_fixture() {
  local root="$1"
  local plugin="$root/bindings/flutter/offline_protocol"
  local rn="$root/bindings/react-native"
  mkdir -p "$plugin" "$rn/android/src/main/jniLibs/arm64-v8a"
  rsync -a --exclude example --exclude .dart_tool \
    "$REPO_ROOT/bindings/flutter/offline_protocol/" "$plugin/"
  echo '{"version":"0.0.0"}' >"$rn/package.json"
  printf '%s\n' 'plugins { id "com.android.library" }' 'compileSdk 34' 'minSdkVersion 24' \
    >"$rn/android/build.gradle"
  touch "$rn/android/consumer-rules.pro"
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    mkdir -p "$rn/android/src/main/jniLibs/$abi"
    echo "so" >"$rn/android/src/main/jniLibs/$abi/libuniffi_offline_protocol.so"
  done
  mkdir -p "$rn/ios/libs/offline_protocol_uniffi.xcframework/ios-arm64"
  echo "a" >"$rn/ios/libs/offline_protocol_uniffi.xcframework/ios-arm64/liboffline_protocol_uniffi.a"
  mkdir -p "$rn/android/src/main/java/uniffi/offline_protocol"
  mkdir -p "$rn/ios/Generated"
  echo "kt" >"$rn/android/src/main/java/uniffi/offline_protocol/offline_protocol.kt"
  echo "// swift" >"$rn/ios/Generated/offline_protocol.swift"
  echo "swift" >"$rn/ios/offline_protocol.swift"
  for legal in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md; do
    echo "$legal" >"$root/$legal"
  done
}

echo "== assemble refuses a non-empty output directory =="
mkdir -p "$WORK/blocked"
echo "x" >"$WORK/blocked/extra"
if PACKAGE_SOURCE_ROOT="$WORK/fixture-refuse" bash "$ASSEMBLE" \
  --output "$WORK/blocked" --version 1.0.0 >"$WORK/refuse.log" 2>&1; then
  fail "non-empty output was accepted"
else
  pass "non-empty output refused"
fi

echo "== success on a fixture tree =="
FIX="$WORK/fixture-ok"
build_fixture "$FIX"
OUT="$WORK/out-ok"
if PACKAGE_SOURCE_ROOT="$FIX" bash "$ASSEMBLE" --output "$OUT" --version 1.2.3 \
  >"$WORK/ok.log" 2>&1; then
  pass "assembled"
else
  fail "assemble failed"
  cat "$WORK/ok.log" >&2
fi
if [ -f "$OUT/android/bridge-rn/build.gradle" ] && \
  [ -f "$OUT/ios/BridgeRN/Generated/offline_protocol.swift" ] && \
  grep -q 'publish_to: https://pub.dev' "$OUT/pubspec.yaml" && \
  grep -q 'version: 1.2.3' "$OUT/pubspec.yaml"; then
  pass "layout and pub metadata"
else
  fail "assembled tree missing expected publish layout"
fi

if [ "$FAILURES" -eq 0 ]; then
  echo "All Flutter pub package assemble checks passed."
else
  echo "$FAILURES check(s) failed." >&2
  exit 1
fi
