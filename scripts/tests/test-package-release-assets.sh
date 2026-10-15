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

# A prerelease, so the wheels' Python spelling (9.9.9rc1) differs from it.
TEST_VERSION="9.9.9-rc.1"
TEST_PY_VERSION="9.9.9rc1"
TEST_WHEELS=(
  "offline_protocol_sdk-$TEST_PY_VERSION-py3-none-macosx_14_0_arm64.whl"
  "offline_protocol_sdk-$TEST_PY_VERSION-py3-none-manylinux_2_39_x86_64.whl"
  "offline_protocol_sdk-$TEST_PY_VERSION-py3-none-manylinux_2_39_aarch64.whl"
  "offline_protocol_sdk-$TEST_PY_VERSION-py3-none-win_amd64.whl"
)
FAILURES=0

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass() { echo "  ok — $*"; }

fail() {
  echo "  FAIL — $*" >&2
  FAILURES=$((FAILURES + 1))
}

sha_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# The lines the assemble script renders for the binary target, with the url
# the release publishes under (scripts/swiftpm-archive-url.sh) unless given.
write_fixture_package() {
  local root="$1" checksum="$2" url="${3:-}"
  [ -n "$url" ] || url="$(bash "$REPO_ROOT/scripts/swiftpm-archive-url.sh" "$TEST_VERSION")"
  rm -rf "$root/swift-package"
  mkdir -p "$root/swift-package/Sources/OfflineProtocolSDK"
  echo "fixture source" >"$root/swift-package/Sources/OfflineProtocolSDK/Bridge.swift"
  echo "fixture package license" >"$root/swift-package/LICENSE"
  cat >"$root/swift-package/Package.swift" <<MANIFEST
// fixture manifest
        .binaryTarget(
            name: "offline_protocolFFI",
            url: "$url",
            checksum: "$checksum"
        ),
MANIFEST
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
  echo "fixture import library" >"$root/desktop-libs/windows-x86_64/offline_protocol_uniffi.dll.lib"

  mkdir -p "$root/bindings/c/include"
  echo "fixture c header" >"$root/bindings/c/include/offline_protocol.h"

  # The archive as build-ios zips it, and the package assembled against it.
  local swiftpm="$root/swiftpm/offline_protocolFFI.xcframework"
  mkdir -p "$swiftpm/ios-arm64/Headers/offline_protocolFFI"
  echo "fixture swiftpm plist" >"$swiftpm/Info.plist"
  echo "fixture swiftpm slice" >"$swiftpm/ios-arm64/liboffline_protocol_uniffi.a"
  echo "fixture swiftpm modulemap" >"$swiftpm/ios-arm64/Headers/offline_protocolFFI/module.modulemap"
  (cd "$root/swiftpm" && zip -qr offline_protocolFFI.xcframework.zip offline_protocolFFI.xcframework)
  rm -rf "$swiftpm"
  write_fixture_package "$root" "$(sha_of "$root/swiftpm/offline_protocolFFI.xcframework.zip")"

  mkdir -p "$root/python-wheels"
  local wheel
  for wheel in "${TEST_WHEELS[@]}"; do
    echo "fixture $wheel" >"$root/python-wheels/$wheel"
  done
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
  "offline-protocol-$TEST_VERSION-swiftpm-xcframework.zip"
  "offline-protocol-$TEST_VERSION-swift-package.tar.gz"
  "${TEST_WHEELS[@]}"
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

# Swift Package Manager unpacks this archive and looks for one artifact in it,
# so it holds the XCFramework and nothing beside it.
SWIFTPM_ZIP="$ASSETS/offline-protocol-$TEST_VERSION-swiftpm-xcframework.zip"
assert_entry "$SWIFTPM_ZIP" "offline_protocolFFI.xcframework/Info.plist"
assert_entry "$SWIFTPM_ZIP" "offline_protocolFFI.xcframework/ios-arm64/liboffline_protocol_uniffi.a"
assert_entry "$SWIFTPM_ZIP" "offline_protocolFFI.xcframework/ios-arm64/Headers/offline_protocolFFI/module.modulemap"
outside="$(list_entries "$SWIFTPM_ZIP" | grep -v '^offline_protocolFFI\.xcframework/' || true)"
if [ -z "$outside" ]; then
  pass "the SwiftPM archive holds the XCFramework alone"
else
  fail "the SwiftPM archive holds more than the XCFramework: $outside"
fi
# Verbatim: the checksum the package names is of these bytes.
if cmp -s "$SWIFTPM_ZIP" "$GOOD/swiftpm/offline_protocolFFI.xcframework.zip"; then
  pass "the SwiftPM archive is published byte for byte"
else
  fail "the SwiftPM archive was rewritten, so the manifest's checksum no longer names it"
fi

SWIFT_PKG="$ASSETS/offline-protocol-$TEST_VERSION-swift-package.tar.gz"
assert_entry "$SWIFT_PKG" "Package.swift"
assert_entry "$SWIFT_PKG" "Sources/OfflineProtocolSDK/Bridge.swift"
assert_entry "$SWIFT_PKG" "LICENSE"

# The whole point of the desktop archives: a bare cdylib is not usable, so each
# one must carry the bindings that call it and the library under the fixed name
# UniFFI's generated loader looks for.
check_desktop() {
  local archive="$1" lib="$2" loader_name="$3"
  assert_entry "$archive" "$lib"
  assert_entry "$archive" "$loader_name"
  assert_entry "$archive" "offline_protocol.py"
  assert_entry "$archive" "offline_protocolFFI.h"
  assert_entry "$archive" "offline_protocol.h"
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
assert_entry "$ASSETS/offline-protocol-$TEST_VERSION-windows-x86_64.zip" offline_protocol_uniffi.dll.lib

# The README is generated per platform, so its C section is checked line for
# line: a wrong link line ships a binary that links and then cannot load.
readme_of() {
  local archive="$1"
  case "$archive" in
    *.tar.gz) tar -xOzf "$archive" ./README.md 2>/dev/null || tar -xOzf "$archive" README.md ;;
    *.zip) unzip -p "$archive" README.md ;;
  esac
}
assert_readme_line() {
  local archive="$1" line="$2"
  if readme_of "$archive" | grep -qxF -- "$line"; then
    pass "$(basename "$archive") README has: $line"
  else
    fail "$(basename "$archive") README is missing the line: $line"
  fi
}
assert_readme_lacks() {
  local archive="$1" text="$2"
  if readme_of "$archive" | grep -qF -- "$text"; then
    fail "$(basename "$archive") README should not mention: $text"
  else
    pass "$(basename "$archive") README does not mention: $text"
  fi
}
# shellcheck disable=SC2016 # the dollar sign is literal README text
ORIGIN_LINE='cc -I. app.c -L. -loffline_protocol_uniffi -Wl,-rpath,'"'"'$ORIGIN'"'"
for plat in linux-x86_64 linux-aarch64; do
  assert_readme_line "$ASSETS/offline-protocol-$TEST_VERSION-$plat.tar.gz" "$ORIGIN_LINE"
  assert_readme_lacks "$ASSETS/offline-protocol-$TEST_VERSION-$plat.tar.gz" "dll.lib"
done
assert_readme_line "$ASSETS/offline-protocol-$TEST_VERSION-macos-arm64.tar.gz" \
  'cc -I. app.c -L. -loffline_protocol_uniffi -Wl,-rpath,@loader_path'
assert_readme_lacks "$ASSETS/offline-protocol-$TEST_VERSION-macos-arm64.tar.gz" "dll.lib"
WIN_ZIP="$ASSETS/offline-protocol-$TEST_VERSION-windows-x86_64.zip"
assert_readme_line "$WIN_ZIP" 'cl /I. app.c offline_protocol_uniffi.dll.lib'
assert_readme_line "$WIN_ZIP" \
  '| `offline_protocol_uniffi.dll.lib` | The import library an MSVC linker needs to link the DLL. |'
for plat in macos-arm64 linux-x86_64 linux-aarch64 windows-x86_64; do
  archive="$(ls "$ASSETS"/offline-protocol-"$TEST_VERSION"-"$plat".*)"
  assert_readme_line "$archive" \
    "[docs/bridges/c.md](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/v$TEST_VERSION/docs/bridges/c.md)"
  # The import row is empty on every platform but Windows; a stray blank line
  # in its place would end the file table early, before the VERSION row.
  last_row="$(readme_of "$archive" | sed -n '/^| File/,/^$/p' | sed '/^$/d' | tail -n 1)"
  if [ "$last_row" = '| `VERSION` | Version and commit this archive was built from. |' ]; then
    pass "$(basename "$archive") README file table runs unbroken to its VERSION row"
  else
    fail "$(basename "$archive") README file table ends early, at: $last_row"
  fi
done

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
if [ "$manifest_lines" -eq 12 ]; then
  pass "SHA256SUMS.txt covers all eight archives and four wheels"
else
  fail "SHA256SUMS.txt has $manifest_lines lines, expected 12"
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
drop_c_header() { rm -f "${1:?}/bindings/c/include/offline_protocol.h"; }
drop_import_lib() { rm -f "${1:?}/desktop-libs/windows-x86_64/offline_protocol_uniffi.dll.lib"; }
drop_android_abis() { rm -rf "${1:?}/bindings/react-native/android/src/main/jniLibs"; }
drop_swiftpm() { rm -rf "${1:?}/swiftpm"; }
drop_swift_package() { rm -rf "${1:?}/swift-package"; }
# The package was assembled against some other archive.
stale_package_checksum() { write_fixture_package "${1:?}" "$(printf '0%.0s' $(seq 64))"; }
# The package names a url the release does not publish under.
stale_package_url() {
  write_fixture_package "${1:?}" "$(sha_of "$1/swiftpm/offline_protocolFFI.xcframework.zip")" \
    "https://github.com/Offline-Protocol/offline-protocol-sdk/releases/download/v0.0.0/offline-protocol-0.0.0-swiftpm-xcframework.zip"
}
drop_package_sources() { rm -rf "${1:?}/swift-package/Sources"; }
# The archive is not what the manifest expects at the top.
flat_archive() {
  rm -f "${1:?}/swiftpm/offline_protocolFFI.xcframework.zip"
  echo "plist" >"$1/swiftpm/Info.plist"
  (cd "$1/swiftpm" && zip -q offline_protocolFFI.xcframework.zip Info.plist)
  write_fixture_package "$1" "$(sha_of "$1/swiftpm/offline_protocolFFI.xcframework.zip")"
}
drop_wheel() { rm -f "${1:?}/python-wheels/${TEST_WHEELS[3]}"; }
# Two wheels for one platform, and four in all, so the count alone passes:
# both would be uploaded, and Windows would get none.
second_wheel() {
  rm -f "${1:?}/python-wheels/${TEST_WHEELS[3]}"
  echo "older" >"$1/python-wheels/offline_protocol_sdk-$TEST_PY_VERSION-py3-none-manylinux_2_17_x86_64.whl"
}
# The release core, not the release candidate: the number the candidate would burn.
wrong_wheel_version() {
  mv "${1:?}/python-wheels/${TEST_WHEELS[0]}" "$1/python-wheels/offline_protocol_sdk-9.9.9-py3-none-macosx_14_0_arm64.whl"
}
# setuptools' own name for it, before the retag.
untagged_wheel() { echo "any" >"${1:?}/python-wheels/offline_protocol_sdk-$TEST_PY_VERSION-py3-none-any.whl"; }

expect_failure "missing-desktop-lib" drop_desktop_lib
expect_failure "missing-ios-generated" drop_ios_generated
expect_failure "missing-python-module" drop_python_module
expect_failure "missing-export-notice" drop_legal
expect_failure "missing-c-header" drop_c_header
expect_failure "missing-windows-import-library" drop_import_lib
expect_failure "missing-android-abis" drop_android_abis
expect_failure "missing-swiftpm-xcframework" drop_swiftpm
expect_failure "missing-swift-package" drop_swift_package
expect_failure "package-names-another-archive" stale_package_checksum
expect_failure "package-names-another-url" stale_package_url
expect_failure "package-without-sources" drop_package_sources
expect_failure "archive-without-the-xcframework-on-top" flat_archive
expect_failure "missing-wheel" drop_wheel
expect_failure "two-wheels-for-one-platform" second_wheel
expect_failure "wheel-numbered-as-the-release-core" wrong_wheel_version
expect_failure "untagged-wheel" untagged_wheel

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
