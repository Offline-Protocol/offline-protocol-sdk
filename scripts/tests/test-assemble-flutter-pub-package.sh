#!/usr/bin/env bash
# Exercises scripts/assemble-flutter-pub-package.sh on a fixture tree (refusals
# and one success path). The success path needs no NDK: it stubs jniLibs,
# XCFramework, and generated bindings where the layout check looks.
#
# The success path ends in `flutter pub publish --dry-run`, because what pub
# puts in the archive is decided by ignore files the layout check cannot see:
# a tree that passes every file test can still publish without its iOS bridge.
# Needs git, Flutter and CocoaPods on PATH.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ASSEMBLE="$REPO_ROOT/scripts/assemble-flutter-pub-package.sh"
VERIFY_PODSPEC="$REPO_ROOT/scripts/verify-flutter-pub-podspec.sh"

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
  # Tracked, but build nothing for a consumer.
  mkdir -p "$rn/android/src/test/java"
  echo "test" >"$rn/android/src/test/java/BridgeTest.kt"
  echo "eclipse" >"$rn/android/.project"
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    mkdir -p "$rn/android/src/main/jniLibs/$abi"
    echo "so" >"$rn/android/src/main/jniLibs/$abi/libuniffi_offline_protocol.so"
  done
  # A library left from an older build under another name, as a maintainer's
  # jniLibs can hold. Only the library the plugin loads may ship.
  echo "stale" >"$rn/android/src/main/jniLibs/arm64-v8a/liboffline_protocol_uniffi.so"
  local xcf="$rn/ios/libs/offline_protocol_uniffi.xcframework"
  for slice in ios-arm64 ios-arm64_x86_64-simulator; do
    mkdir -p "$xcf/$slice"
    echo "a" >"$xcf/$slice/liboffline_protocol_uniffi.a"
  done
  echo "plist" >"$xcf/Info.plist"
  mkdir -p "$rn/android/src/main/java/uniffi/offline_protocol"
  mkdir -p "$rn/ios/Generated"
  echo "kt" >"$rn/android/src/main/java/uniffi/offline_protocol/offline_protocol.kt"
  echo "// swift" >"$rn/ios/Generated/offline_protocol.swift"
  echo "swift" >"$rn/ios/offline_protocol.swift"
  printf '%s\n' 's.platforms    = { :ios => "13.0" }' >"$rn/MeshSdk.podspec"
  touch "$rn/ios/PrivacyInfo.xcprivacy"
  # The React Native package carries its own ios/.gitignore; pub would honour it.
  printf '%s\n' '.build/' >"$rn/ios/.gitignore"
  for legal in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md; do
    echo "$legal" >"$root/$legal"
  done
  # The package is built from tracked files, as in a checkout. The native
  # libraries are untracked there (the repository ignores *.so and *.a), so
  # they stay untracked here.
  git -C "$root" init -q
  git -C "$root" add -A
  git -C "$root" rm -q -r --cached \
    "bindings/react-native/android/src/main/jniLibs" "bindings/react-native/ios/libs"
  git -C "$root" -c user.name=fixture -c user.email=fixture@invalid \
    commit -q -m fixture
  # What a build or an IDE leaves in a working tree; none of it may ship.
  for junk in \
    bindings/flutter/offline_protocol/android/local.properties \
    bindings/flutter/offline_protocol/.idea/workspace.xml \
    bindings/react-native/android/.cxx/junk \
    bindings/react-native/ios/.build/junk; do
    mkdir -p "$(dirname "$root/$junk")"
    echo junk >"$root/$junk"
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

echo "== assemble refuses an output inside a git work tree =="
mkdir -p "$WORK/repo"
git -C "$WORK/repo" init -q
if PACKAGE_SOURCE_ROOT="$WORK/fixture-refuse" bash "$ASSEMBLE" \
  --output "$WORK/repo/out" --version 1.0.0 >"$WORK/git.log" 2>&1; then
  fail "an output inside a git work tree was accepted"
elif grep -q 'inside a git work tree' "$WORK/git.log"; then
  pass "output inside a git work tree refused"
else
  fail "refused for another reason: $(cat "$WORK/git.log")"
fi

echo "== assemble refuses a native file it does not expect =="
FIX_STRAY="$WORK/fixture-stray"
build_fixture "$FIX_STRAY"
echo "stray" >"$FIX_STRAY/bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework/ios-arm64/stray.dylib"
if PACKAGE_SOURCE_ROOT="$FIX_STRAY" bash "$ASSEMBLE" --output "$WORK/out-stray" --version 1.0.0 \
  >"$WORK/stray.log" 2>&1; then
  fail "a stray file in the XCFramework was accepted"
elif grep -q 'unexpected native file: ios/Frameworks/offline_protocol_uniffi.xcframework/ios-arm64/stray.dylib' "$WORK/stray.log"; then
  pass "stray native file refused"
else
  fail "refused for another reason: $(cat "$WORK/stray.log")"
fi

echo "== assemble refuses an XCFramework without the simulator slice =="
FIX_SIM="$WORK/fixture-no-sim"
build_fixture "$FIX_SIM"
rm -rf "$FIX_SIM/bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework/ios-arm64_x86_64-simulator"
if PACKAGE_SOURCE_ROOT="$FIX_SIM" bash "$ASSEMBLE" --output "$WORK/out-no-sim" --version 1.0.0 \
  >"$WORK/no-sim.log" 2>&1; then
  fail "an XCFramework without the simulator slice was accepted"
elif grep -q 'missing: ios/Frameworks/offline_protocol_uniffi.xcframework/ios-arm64_x86_64-simulator/' "$WORK/no-sim.log"; then
  pass "missing simulator slice refused"
else
  fail "refused for another reason: $(cat "$WORK/no-sim.log")"
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
  [ -f "$OUT/ios/pub_bridge_version.json" ] && \
  grep -q '"ios_deployment_target":"13.0"' "$OUT/ios/pub_bridge_version.json" && \
  grep -q 'publish_to: https://pub.dev' "$OUT/pubspec.yaml" && \
  grep -q 'version: 1.2.3' "$OUT/pubspec.yaml"; then
  pass "layout and pub metadata"
else
  fail "assembled tree missing expected publish layout"
fi
if [ -f "$OUT/android/src/main/jniLibs/arm64-v8a/libuniffi_offline_protocol.so" ] && \
  [ -f "$OUT/ios/Frameworks/offline_protocol_uniffi.xcframework/ios-arm64/liboffline_protocol_uniffi.a" ] && \
  [ -f "$OUT/ios/Frameworks/offline_protocol_uniffi.xcframework/ios-arm64_x86_64-simulator/liboffline_protocol_uniffi.a" ] && \
  [ ! -e "$OUT/android/bridge-rn/src/main/jniLibs" ] && \
  [ ! -e "$OUT/ios/BridgeRN/libs" ]; then
  pass "each native library ships once, from the staged locations"
else
  fail "native libraries missing from the staged locations or duplicated in the vendored bridge"
fi
if [ ! -e "$OUT/android/bridge-rn/src/test" ] && [ ! -e "$OUT/android/bridge-rn/.project" ]; then
  pass "the vendored Android bridge leaves out its tests and project file"
else
  fail "the vendored Android bridge carries src/test or .project"
fi
if [ -z "$(find "$OUT" -name liboffline_protocol_uniffi.so)" ]; then
  pass "a stale library beside the real one stays out of the package"
else
  fail "a stale library from the source jniLibs was copied into the package"
fi
if [ -z "$(find "$OUT" \( -name local.properties -o -name .idea -o -name .cxx -o -name .build \))" ]; then
  pass "untracked files in the source tree stay out of the package"
else
  fail "untracked files from the source tree were copied into the package"
fi
if [ -z "$(find "$OUT" \( -name .gitignore -o -name .pubignore \) -not -path "$OUT/.pubignore")" ]; then
  pass "no ignore file but the root .pubignore ships"
else
  fail "an ignore file ships in the tree and would hide files from pub"
fi
if [ -f "$OUT/README.md" ] && grep -q '^## 1.2.3$' "$OUT/CHANGELOG.md"; then
  pass "README and a CHANGELOG naming the version"
else
  fail "README.md or CHANGELOG.md missing, or the CHANGELOG does not name 1.2.3"
fi
if bash "$VERIFY_PODSPEC" "$OUT" >"$WORK/pod.log" 2>&1; then
  pass "iOS podspec evaluates (pod ipc spec)"
else
  fail "podspec verification failed"
  cat "$WORK/pod.log" >&2
fi

echo "== pub publish --dry-run on the assembled tree =="
if (cd "$OUT" && flutter pub publish --dry-run) >"$WORK/dry.log" 2>&1; then
  pass "dry-run publish passes with no errors or warnings"
else
  fail "dry-run publish failed"
  cat "$WORK/dry.log" >&2
fi
# The archive listing is a tree, so match the names that only appear where the
# iOS bridge, the staged framework and the staged libraries live.
for entry in BridgeRN Generated Frameworks liboffline_protocol_uniffi.a \
  libuniffi_offline_protocol.so pub_bridge_version.json README.md CHANGELOG.md LICENSE; do
  # Anchored to a tree line, so pub's own "Please add a CHANGELOG.md" warning
  # does not count as the file.
  if grep -qE -- "── ${entry//./\\.}( |\$)" "$WORK/dry.log"; then
    pass "archive carries $entry"
  else
    fail "archive is missing $entry"
  fi
done

if [ "$FAILURES" -eq 0 ]; then
  echo "All Flutter pub package assemble checks passed."
else
  echo "$FAILURES check(s) failed." >&2
  exit 1
fi
