#!/usr/bin/env bash

# Package the compiled binaries into the archives attached to a GitHub release.
#
# Called by .github/workflows/release.yml once the per-platform build artifacts
# have been downloaded into the working tree.
#
# This lives in a script rather than inline in the workflow because release.yml
# only ever runs on a `v*` tag: an inline version's first real execution would be
# during a release, with no way to shellcheck it, run it locally, or exercise its
# failure paths. scripts/tests/test-package-release-assets.sh drives it against a
# fixture tree on every PR instead.
#
# Usage:
#   VERSION=0.21.0 bash scripts/package-release-assets.sh
#
# Environment:
#   VERSION              required — e.g. 0.21.0
#   GITHUB_SHA           optional — recorded in the VERSION file inside each archive
#   PACKAGE_SOURCE_ROOT  optional — defaults to the repository root
#   PACKAGE_OUTPUT_DIR   optional — defaults to <source root>/release-assets

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
ASSETS="${PACKAGE_OUTPUT_DIR:-$ROOT/release-assets}"

: "${VERSION:?VERSION must be set, e.g. VERSION=0.21.0}"

RN="$ROOT/bindings/react-native"
IOS_XCFRAMEWORK="$RN/ios/libs/offline_protocol_uniffi.xcframework"
IOS_GENERATED="$RN/ios/Generated"
# The C scaffolding header UniFFI generates. It is emitted under ios/Generated/
# but it is not iOS-specific — it is the only header a C or C++ consumer of the
# desktop cdylib has to work from, so the desktop archives carry it too.
FFI_HEADER="$IOS_GENERATED/offline_protocolFFI.h"
ANDROID_JNILIBS="$RN/android/src/main/jniLibs"
ANDROID_KOTLIN="$RN/android/src/main/java/uniffi"
PYTHON_MODULE="$ROOT/bindings/python/offline_protocol_sdk/offline_protocol.py"
DESKTOP_LIBS="$ROOT/desktop-libs"

# Every archive carries the licences and the export notice with it. A binary
# handed out on its own is an AGPL distribution, and 15 CFR §742.15(b) attaches
# to the cryptography, not to the npm package that usually delivers it — so an
# asset downloaded straight off the release page must not arrive without either.
LEGAL_FILES=(LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md)

DESKTOP_PLATFORMS=(macos-arm64 linux-x86_64 linux-aarch64 windows-x86_64)

die() {
  echo "ERROR: $*" >&2
  exit 1
}

require_file() {
  [ -f "$1" ] || die "required file is missing: ${1#"$ROOT"/}"
}

require_nonempty_dir() {
  [ -d "$1" ] || die "required directory is missing: ${1#"$ROOT"/}"
  [ -n "$(find "$1" -mindepth 1 -maxdepth 1 -print -quit)" ] ||
    die "required directory is empty: ${1#"$ROOT"/}"
}

# sha256sum is coreutils (Linux); macOS ships shasum. Both emit "<hash>  <name>".
sha256_manifest() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$@"
  else
    shasum -a 256 -- "$@"
  fi
}

# Resolves the per-platform desktop naming. Used twice: once in pre-flight to
# check every input before anything is written, once while packaging.
desktop_spec() {
  case "$1" in
    macos-*)
      lib=liboffline_protocol_uniffi.dylib
      loader_name=libuniffi.dylib
      fmt=tar
      built_on="macOS 14 (arm64)"
      ;;
    linux-*)
      lib=liboffline_protocol_uniffi.so
      loader_name=libuniffi.so
      fmt=tar
      built_on="ubuntu-latest — the library carries that image's glibc floor"
      ;;
    windows-*)
      lib=offline_protocol_uniffi.dll
      loader_name=uniffi.dll
      fmt=zip
      built_on="windows-latest (MSVC toolchain)"
      ;;
    *)
      die "unhandled desktop platform: $1"
      ;;
  esac
}

# ---------------------------------------------------------------------------
# Pre-flight
#
# Checked by name up front so a missing input names itself, rather than
# surfacing as a bare `cp` error partway through a half-built asset set.
# ---------------------------------------------------------------------------

LEGAL_PATHS=()
for legal in "${LEGAL_FILES[@]}"; do
  require_file "$ROOT/$legal"
  LEGAL_PATHS+=("$ROOT/$legal")
done

require_file "$IOS_XCFRAMEWORK/Info.plist"
require_nonempty_dir "$IOS_GENERATED"
require_file "$FFI_HEADER"
require_nonempty_dir "$ANDROID_JNILIBS"
require_nonempty_dir "$ANDROID_KOTLIN"
require_file "$PYTHON_MODULE"

for plat in "${DESKTOP_PLATFORMS[@]}"; do
  desktop_spec "$plat"
  require_file "$DESKTOP_LIBS/$plat/$lib"
done

EXPECTED_ASSETS=(
  "offline-protocol-$VERSION-ios-xcframework.zip"
  "offline-protocol-$VERSION-android.zip"
  "offline-protocol-$VERSION-macos-arm64.tar.gz"
  "offline-protocol-$VERSION-linux-x86_64.tar.gz"
  "offline-protocol-$VERSION-linux-aarch64.tar.gz"
  "offline-protocol-$VERSION-windows-x86_64.zip"
)

STAGE_ROOT="$(mktemp -d)"
PACKAGING_COMPLETE=0

# A partial asset set is the one failure mode that would actually publish:
# release.yml uploads release-assets/* verbatim. Pre-flight above makes a
# missing input fail before the first archive is written; this makes the
# directory empty-or-complete even when the failure is something pre-flight
# cannot see (out of disk, a killed job). Only the exact names this script
# writes are removed, so a caller-supplied output directory is never at risk.
cleanup() {
  rm -rf "$STAGE_ROOT"
  if [ "$PACKAGING_COMPLETE" -eq 0 ]; then
    rm -f "$ASSETS/SHA256SUMS.txt"
    for stale in "${EXPECTED_ASSETS[@]}"; do
      rm -f "$ASSETS/$stale"
    done
  fi
}
trap cleanup EXIT

mkdir -p "$ASSETS"
# Clear the exact names this run writes, so a local re-run is idempotent
# without ever handing `rm -rf` a caller-supplied directory.
rm -f "$ASSETS/SHA256SUMS.txt"
for asset in "${EXPECTED_ASSETS[@]}"; do
  rm -f "$ASSETS/$asset"
done

# ---------------------------------------------------------------------------
# Staging helpers
# ---------------------------------------------------------------------------

# Drops the legal files and a VERSION stamp into a staging directory. The stamp
# is what makes an *extracted* tree identifiable: the version otherwise lives
# only in the archive filename, which is exactly the wrong place for forensics
# on a release that already shipped.
seed_common() {
  local stage="$1" asset="$2"
  cp "${LEGAL_PATHS[@]}" "$stage/"
  {
    printf 'name=offline-protocol-sdk\n'
    printf 'version=%s\n' "$VERSION"
    printf 'commit=%s\n' "${GITHUB_SHA:-unknown}"
    printf 'asset=%s\n' "$asset"
  } >"$stage/VERSION"
}

archive_zip() {
  local stage="$1" out="$2"
  (cd "$stage" && zip -qry "$out" .)
}

archive_tar() {
  local stage="$1" out="$2"
  tar -czf "$out" -C "$stage" .
}

new_stage() {
  local stage="$STAGE_ROOT/$1"
  mkdir -p "$stage"
  printf '%s' "$stage"
}

# ---------------------------------------------------------------------------
# iOS
#
# The XCFramework alone does not build: the generated FFI header, modulemap and
# Swift bindings under Generated/ are what an importer actually needs, and npm is
# the only place they ship today.
# ---------------------------------------------------------------------------

stage="$(new_stage ios)"
cp -R "$IOS_XCFRAMEWORK" "$stage/"
cp -R "$IOS_GENERATED" "$stage/"
seed_common "$stage" "offline-protocol-$VERSION-ios-xcframework.zip"
archive_zip "$stage" "$ASSETS/offline-protocol-$VERSION-ios-xcframework.zip"

# ---------------------------------------------------------------------------
# Android: all four ABIs plus the Kotlin bindings that call them.
# ---------------------------------------------------------------------------

stage="$(new_stage android)"
mkdir -p "$stage/jniLibs"
cp -R "$ANDROID_JNILIBS/." "$stage/jniLibs/"
cp -R "$ANDROID_KOTLIN" "$stage/"
seed_common "$stage" "offline-protocol-$VERSION-android.zip"
archive_zip "$stage" "$ASSETS/offline-protocol-$VERSION-android.zip"

# ---------------------------------------------------------------------------
# Desktop: zip for Windows, tar.gz elsewhere, matching what each platform's
# tooling expects to be handed.
#
# The same rule the iOS archive follows applies here: a bare cdylib is not
# usable. Each desktop archive therefore carries the generated Python module
# that calls into it and the C scaffolding header, alongside the library under
# both its build name and the fixed name UniFFI's codegen loads.
# ---------------------------------------------------------------------------

for plat in "${DESKTOP_PLATFORMS[@]}"; do
  desktop_spec "$plat"

  if [ "$fmt" = tar ]; then
    asset_name="offline-protocol-$VERSION-$plat.tar.gz"
  else
    asset_name="offline-protocol-$VERSION-$plat.zip"
  fi

  stage="$(new_stage "$plat")"
  cp "$DESKTOP_LIBS/$plat/$lib" "$stage/"
  cp "$PYTHON_MODULE" "$stage/"
  cp "$FFI_HEADER" "$stage/"

  # _uniffi_load_indirect() in offline_protocol.py loads the library by the
  # fixed name UniFFI bakes into its codegen, not by the cargo output name.
  # Mirrors what bindings/python/scripts/build-desktop.sh stages locally: a
  # symlink where the archive format preserves one, a copy on Windows where
  # extractors will not create one.
  if [ "$fmt" = tar ]; then
    ln -s "$lib" "$stage/$loader_name"
  else
    cp "$stage/$lib" "$stage/$loader_name"
  fi

  cat >"$stage/README.md" <<EOF
# Offline Protocol SDK — $plat native library

Version $VERSION. Built on $built_on.

## Contents

| File | Purpose |
|---|---|
| \`$lib\` | The native library, under its build name. |
| \`$loader_name\` | The same library under the name the generated bindings load. |
| \`offline_protocol.py\` | Generated Python bindings. Import from this directory. |
| \`offline_protocolFFI.h\` | C scaffolding header, for consumers linking directly. |
| \`VERSION\` | Version and commit this archive was built from. |

## Python

Keep \`offline_protocol.py\` and \`$loader_name\` in the same directory — the
generated loader resolves the library relative to its own file — then:

\`\`\`python
import offline_protocol
\`\`\`

## Licensing

Distributed under the AGPL-3.0 terms in \`LICENSE\`; see \`LICENSE-COMMERCIAL.md\`
for the alternative. \`THIRD-PARTY-NOTICES.md\` covers the bundled dependencies
and \`EXPORT.md\` carries the 15 CFR §742.15(b) notice that attaches to the
cryptography in this library.
EOF

  seed_common "$stage" "$asset_name"

  if [ "$fmt" = tar ]; then
    archive_tar "$stage" "$ASSETS/$asset_name"
  else
    archive_zip "$stage" "$ASSETS/$asset_name"
  fi
done

# ---------------------------------------------------------------------------
# Manifest and final assertions
# ---------------------------------------------------------------------------

for asset in "${EXPECTED_ASSETS[@]}"; do
  [ -s "$ASSETS/$asset" ] || die "asset was not produced or is empty: $asset"
done

# Written outside the directory and moved in, so the manifest can never race its
# own glob. Hashed by explicit name rather than a glob so the manifest provably
# covers the intended set.
manifest_tmp="${RUNNER_TEMP:-$STAGE_ROOT}/SHA256SUMS.txt"
(cd "$ASSETS" && sha256_manifest "${EXPECTED_ASSETS[@]}") >"$manifest_tmp"
mv "$manifest_tmp" "$ASSETS/SHA256SUMS.txt"

# release.yml uploads release-assets/* verbatim, so a stray file left by an
# earlier run would be published alongside the real ones. Refuse rather than
# ship it.
printf '%s\n' "${EXPECTED_ASSETS[@]}" SHA256SUMS.txt >"$STAGE_ROOT/expected-listing.txt"
unexpected=""
while IFS= read -r found; do
  name="${found##*/}"
  grep -qxF "$name" "$STAGE_ROOT/expected-listing.txt" || unexpected+="  $name"$'\n'
done < <(find "$ASSETS" -mindepth 1 -maxdepth 1)
if [ -n "$unexpected" ]; then
  echo "ERROR: unexpected files in $ASSETS — every file here is uploaded to the release:" >&2
  echo "$unexpected" >&2
  exit 1
fi

PACKAGING_COMPLETE=1

echo "Release assets:"
ls -lh "$ASSETS"
