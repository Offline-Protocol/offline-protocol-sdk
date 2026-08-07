#!/usr/bin/env bash

# Drives scripts/package-release-assets.sh against a fixture tree that mirrors
# what release.yml's download steps produce.
#
# release.yml only runs on a `v*` tag, so without this the packaging script's
# first execution would be during a real release. This asserts the asset set,
# the internal layout of every archive, the manifest, and — the part that
# matters most — that a missing input fails the job rather than producing a
# partial asset set that publishes.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PACKAGER="$REPO_ROOT/scripts/package-release-assets.sh"

TEST_VERSION="9.9.9-test"
FAILURES=0

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass() { echo "  ok — $*"; }

fail() {
  echo "  FAIL — $*" >&2
  FAILURES=$((FAILURES + 1))
}

# Builds a complete, valid fixture tree at $1.
build_fixture() {
  local root="$1"
  local rn="$root/bindings/react-native"

  mkdir -p "$root"
  for legal in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md; do
    echo "fixture $legal" >"$root/$legal"
  done

  local xcf="$rn/ios/libs/offline_protocol_uniffi.xcframework"
  mkdir -p "$xcf/ios-arm64" "$xcf/ios-arm64_x86_64-simulator"
  echo "fixture plist" >"$xcf/Info.plist"
  echo "fixture device slice" >"$xcf/ios-arm64/liboffline_protocol_uniffi.a"
  echo "fixture sim slice" >"$xcf/ios-arm64_x86_64-simulator/liboffline_protocol_uniffi.a"

  mkdir -p "$rn/ios/Generated"
  echo "fixture swift" >"$rn/ios/Generated/offline_protocol.swift"
  echo "fixture header" >"$rn/ios/Generated/offline_protocolFFI.h"
  echo "fixture modulemap" >"$rn/ios/Generated/offline_protocolFFI.modulemap"

  for abi in arm64-v8a armeabi-v7a x86_64 x86; do
    mkdir -p "$rn/android/src/main/jniLibs/$abi"
    echo "fixture $abi" >"$rn/android/src/main/jniLibs/$abi/libuniffi_offline_protocol.so"
  done
  mkdir -p "$rn/android/src/main/java/uniffi/offline_protocol"
  echo "fixture kotlin" >"$rn/android/src/main/java/uniffi/offline_protocol/offline_protocol.kt"

  mkdir -p "$root/bindings/python/offline_protocol_sdk"
  echo "fixture python bindings" >"$root/bindings/python/offline_protocol_sdk/offline_protocol.py"

  mkdir -p "$root/desktop-libs/macos-arm64" \
    "$root/desktop-libs/linux-x86_64" \
    "$root/desktop-libs/linux-aarch64" \
    "$root/desktop-libs/windows-x86_64"
  echo "fixture dylib" >"$root/desktop-libs/macos-arm64/liboffline_protocol_uniffi.dylib"
  echo "fixture so x86_64" >"$root/desktop-libs/linux-x86_64/liboffline_protocol_uniffi.so"
  echo "fixture so aarch64" >"$root/desktop-libs/linux-aarch64/liboffline_protocol_uniffi.so"
  echo "fixture dll" >"$root/desktop-libs/windows-x86_64/offline_protocol_uniffi.dll"
}

run_packager() {
  local root="$1"
  env -u RUNNER_TEMP \
    VERSION="$TEST_VERSION" \
    GITHUB_SHA=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef \
    PACKAGE_SOURCE_ROOT="$root" \
    PACKAGE_OUTPUT_DIR="$root/release-assets" \
    bash "$PACKAGER"
}

# Lists archive entries, normalised to paths without a leading ./
list_entries() {
  local archive="$1"
  case "$archive" in
    *.tar.gz) tar -tzf "$archive" ;;
    *.zip) unzip -Z1 "$archive" ;;
  esac | sed 's|^\./||' | sed '/^$/d'
}

assert_entry() {
  local archive="$1" entry="$2"
  if list_entries "$archive" | grep -qxF "$entry"; then
    pass "$(basename "$archive") contains $entry"
  else
    fail "$(basename "$archive") is missing $entry"
  fi
}

# ---------------------------------------------------------------------------
echo "== happy path =="
# ---------------------------------------------------------------------------

GOOD="$WORK/good"
build_fixture "$GOOD"
if run_packager "$GOOD" >"$WORK/good.log" 2>&1; then
  pass "packaging succeeded"
else
  fail "packaging failed on a complete fixture tree"
  cat "$WORK/good.log" >&2
  exit 1
fi

ASSETS="$GOOD/release-assets"

EXPECTED=(
  "offline-protocol-$TEST_VERSION-ios-xcframework.zip"
  "offline-protocol-$TEST_VERSION-android.zip"
  "offline-protocol-$TEST_VERSION-macos-arm64.tar.gz"
  "offline-protocol-$TEST_VERSION-linux-x86_64.tar.gz"
  "offline-protocol-$TEST_VERSION-linux-aarch64.tar.gz"
  "offline-protocol-$TEST_VERSION-windows-x86_64.zip"
  SHA256SUMS.txt
)

actual_count="$(find "$ASSETS" -maxdepth 1 -type f | wc -l | tr -d ' ')"
if [ "$actual_count" -eq "${#EXPECTED[@]}" ]; then
  pass "produced exactly ${#EXPECTED[@]} assets"
else
  fail "expected ${#EXPECTED[@]} assets, found $actual_count"
  ls -A "$ASSETS" >&2
fi

for asset in "${EXPECTED[@]}"; do
  if [ -s "$ASSETS/$asset" ]; then
    pass "$asset present and non-empty"
  else
    fail "$asset missing or empty"
  fi
done

# ---------------------------------------------------------------------------
echo "== archive layout =="
# ---------------------------------------------------------------------------

IOS_ZIP="$ASSETS/offline-protocol-$TEST_VERSION-ios-xcframework.zip"
assert_entry "$IOS_ZIP" "offline_protocol_uniffi.xcframework/Info.plist"
assert_entry "$IOS_ZIP" "offline_protocol_uniffi.xcframework/ios-arm64/liboffline_protocol_uniffi.a"
assert_entry "$IOS_ZIP" "offline_protocol_uniffi.xcframework/ios-arm64_x86_64-simulator/liboffline_protocol_uniffi.a"
assert_entry "$IOS_ZIP" "Generated/offline_protocolFFI.h"
assert_entry "$IOS_ZIP" "Generated/offline_protocolFFI.modulemap"
assert_entry "$IOS_ZIP" "Generated/offline_protocol.swift"

ANDROID_ZIP="$ASSETS/offline-protocol-$TEST_VERSION-android.zip"
for abi in arm64-v8a armeabi-v7a x86_64 x86; do
  assert_entry "$ANDROID_ZIP" "jniLibs/$abi/libuniffi_offline_protocol.so"
done
assert_entry "$ANDROID_ZIP" "uniffi/offline_protocol/offline_protocol.kt"

# The whole point of the desktop archives: a bare cdylib is not usable, so each
# one must carry the bindings that call it and the library under the fixed name
# UniFFI's generated loader looks for.
check_desktop() {
  local archive="$1" lib="$2" loader_name="$3"
  assert_entry "$archive" "$lib"
  assert_entry "$archive" "$loader_name"
  assert_entry "$archive" "offline_protocol.py"
  assert_entry "$archive" "offline_protocolFFI.h"
  assert_entry "$archive" "VERSION"
  assert_entry "$archive" "README.md"
  assert_entry "$archive" "LICENSE"
  assert_entry "$archive" "EXPORT.md"
}

check_desktop "$ASSETS/offline-protocol-$TEST_VERSION-macos-arm64.tar.gz" \
  liboffline_protocol_uniffi.dylib libuniffi.dylib
check_desktop "$ASSETS/offline-protocol-$TEST_VERSION-linux-x86_64.tar.gz" \
  liboffline_protocol_uniffi.so libuniffi.so
check_desktop "$ASSETS/offline-protocol-$TEST_VERSION-linux-aarch64.tar.gz" \
  liboffline_protocol_uniffi.so libuniffi.so
check_desktop "$ASSETS/offline-protocol-$TEST_VERSION-windows-x86_64.zip" \
  offline_protocol_uniffi.dll uniffi.dll

# Windows extractors do not create symlinks, so uniffi.dll has to be a real copy
# of the DLL rather than a link that arrives broken.
WIN_DIR="$WORK/win-extract"
mkdir -p "$WIN_DIR"
unzip -qq "$ASSETS/offline-protocol-$TEST_VERSION-windows-x86_64.zip" -d "$WIN_DIR"
if [ -f "$WIN_DIR/uniffi.dll" ] && [ ! -L "$WIN_DIR/uniffi.dll" ]; then
  pass "windows uniffi.dll is a real file, not a symlink"
else
  fail "windows uniffi.dll must be a copy — extractors will not create symlinks"
fi

# On tar targets the alias is a symlink, so it costs nothing.
TAR_DIR="$WORK/linux-extract"
mkdir -p "$TAR_DIR"
tar -xzf "$ASSETS/offline-protocol-$TEST_VERSION-linux-x86_64.tar.gz" -C "$TAR_DIR"
if [ -L "$TAR_DIR/libuniffi.so" ]; then
  pass "linux libuniffi.so is a symlink"
else
  fail "linux libuniffi.so should be a symlink, not a duplicated library"
fi
if [ -f "$TAR_DIR/libuniffi.so" ]; then
  pass "linux libuniffi.so resolves to the library"
else
  fail "linux libuniffi.so is a dangling symlink"
fi

if grep -qxF "version=$TEST_VERSION" "$TAR_DIR/VERSION" &&
  grep -q '^commit=deadbeef' "$TAR_DIR/VERSION"; then
  pass "VERSION stamp carries the version and commit"
else
  fail "VERSION stamp is missing the version or commit"
  cat "$TAR_DIR/VERSION" >&2
fi

# ---------------------------------------------------------------------------
echo "== manifest =="
# ---------------------------------------------------------------------------

manifest_lines="$(wc -l <"$ASSETS/SHA256SUMS.txt" | tr -d ' ')"
if [ "$manifest_lines" -eq 6 ]; then
  pass "SHA256SUMS.txt covers all six archives"
else
  fail "SHA256SUMS.txt has $manifest_lines lines, expected 6"
fi

if grep -q "SHA256SUMS.txt" "$ASSETS/SHA256SUMS.txt"; then
  fail "SHA256SUMS.txt lists itself"
else
  pass "SHA256SUMS.txt does not list itself"
fi

if (cd "$ASSETS" && if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c --quiet SHA256SUMS.txt
else
  shasum -a 256 -c -s SHA256SUMS.txt
fi); then
  pass "every recorded checksum verifies"
else
  fail "checksum verification failed"
fi

# ---------------------------------------------------------------------------
echo "== negative controls =="
# ---------------------------------------------------------------------------

# Each removes one required input and asserts a non-zero exit with no assets
# written. A partial asset set is the failure mode that would actually publish.
expect_failure() {
  local label="$1" mutate="$2"
  local root="$WORK/neg-$label"
  build_fixture "$root"
  "$mutate" "$root"
  if run_packager "$root" >"$WORK/neg-$label.log" 2>&1; then
    fail "$label: packaging succeeded but should have failed"
  else
    # Guarded rather than piped through `find ... || true`: under `set -o
    # pipefail` a find over a directory the packager never created would fail
    # the harness itself instead of the case under test.
    local produced=0
    if [ -d "$root/release-assets" ]; then
      produced="$(find "$root/release-assets" -maxdepth 1 -type f | wc -l | tr -d ' ')"
    fi
    if [ "$produced" -eq 0 ]; then
      pass "$label: failed with no assets written"
    else
      fail "$label: failed but left $produced asset(s) behind"
    fi
  fi
}

drop_desktop_lib() { rm -rf "${1:?}/desktop-libs/linux-aarch64"; }
drop_ios_generated() { rm -rf "${1:?}/bindings/react-native/ios/Generated"; }
drop_python_module() { rm -f "${1:?}/bindings/python/offline_protocol_sdk/offline_protocol.py"; }
drop_legal() { rm -f "${1:?}/EXPORT.md"; }
drop_android_abis() { rm -rf "${1:?}/bindings/react-native/android/src/main/jniLibs"; }

expect_failure "missing-desktop-lib" drop_desktop_lib
expect_failure "missing-ios-generated" drop_ios_generated
expect_failure "missing-python-module" drop_python_module
expect_failure "missing-export-notice" drop_legal
expect_failure "missing-android-abis" drop_android_abis

# VERSION is required — an unset one must not silently produce
# "offline-protocol--linux-x86_64.tar.gz".
NOVER="$WORK/neg-no-version"
build_fixture "$NOVER"
if env -u VERSION -u RUNNER_TEMP \
  PACKAGE_SOURCE_ROOT="$NOVER" \
  PACKAGE_OUTPUT_DIR="$NOVER/release-assets" \
  bash "$PACKAGER" >"$WORK/neg-no-version.log" 2>&1; then
  fail "missing-version: packaging succeeded without VERSION set"
else
  pass "missing-version: failed with no VERSION set"
fi

# A stray file in the output directory would be uploaded verbatim by
# release.yml's `files: release-assets/*`.
STRAY="$WORK/neg-stray"
build_fixture "$STRAY"
mkdir -p "$STRAY/release-assets"
echo "leftover" >"$STRAY/release-assets/not-an-asset.txt"
if run_packager "$STRAY" >"$WORK/neg-stray.log" 2>&1; then
  fail "stray-file: packaging succeeded with an unexpected file in the output dir"
else
  pass "stray-file: refused to publish alongside an unexpected file"
fi

# ---------------------------------------------------------------------------
echo "== idempotence =="
# ---------------------------------------------------------------------------

# A re-run must overwrite cleanly rather than trip its own stray-file check.
if run_packager "$GOOD" >"$WORK/rerun.log" 2>&1; then
  pass "re-running over an existing asset set succeeds"
else
  fail "re-running over an existing asset set failed"
  cat "$WORK/rerun.log" >&2
fi

# ---------------------------------------------------------------------------

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "All release-packaging checks passed."
else
  echo "$FAILURES release-packaging check(s) failed." >&2
  exit 1
fi
