#!/bin/bash

# Prepare for npm publish
# Validates that all required binaries are present before publishing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RN_DIR="$SCRIPT_DIR/.."

echo "Validating pre-built binaries for npm package..."
echo ""

# Check the iOS XCFramework. Both slices are validated individually: a bundle
# carrying only the device slice still installs and still builds for a device,
# so a missing simulator slice would otherwise ship silently and read to
# consumers as "this SDK doesn't support the simulator".
# Package-relative, so the on-disk checks and the packlist check below cannot
# name different bundles.
IOS_XCFRAMEWORK_REL="ios/libs/offline_protocol_uniffi.xcframework"
IOS_XCFRAMEWORK="$RN_DIR/$IOS_XCFRAMEWORK_REL"
IOS_DEVICE_SLICE="$IOS_XCFRAMEWORK/ios-arm64/liboffline_protocol_uniffi.a"
IOS_SIM_SLICE="$IOS_XCFRAMEWORK/ios-arm64_x86_64-simulator/liboffline_protocol_uniffi.a"

if [ -f "$IOS_XCFRAMEWORK/Info.plist" ]; then
  echo "✅ iOS XCFramework found: $IOS_XCFRAMEWORK"
  echo "   Size: $(du -sh "$IOS_XCFRAMEWORK" | cut -f1)"
else
  echo "❌ iOS XCFramework missing: $IOS_XCFRAMEWORK"
  echo "   Run: npm run build:ios"
  exit 1
fi

if [ -f "$IOS_DEVICE_SLICE" ]; then
  echo "✅ iOS device slice found"
  echo "   Architectures: $(lipo -info "$IOS_DEVICE_SLICE" | sed 's/.*: //')"
else
  echo "❌ iOS device slice missing: $IOS_DEVICE_SLICE"
  echo "   Run: npm run build:ios"
  exit 1
fi

if [ -f "$IOS_SIM_SLICE" ]; then
  echo "✅ iOS simulator slice found"
  echo "   Architectures: $(lipo -info "$IOS_SIM_SLICE" | sed 's/.*: //')"
else
  echo "❌ iOS simulator slice missing: $IOS_SIM_SLICE"
  echo "   Run: npm run build:ios"
  exit 1
fi

echo ""

# Check Android libraries
ANDROID_DIR="$RN_DIR/android/src/main/jniLibs"
ANDROID_ABIS=("arm64-v8a" "armeabi-v7a" "x86_64" "x86")
MISSING_ABIS=()

for abi in "${ANDROID_ABIS[@]}"; do
  lib_path="$ANDROID_DIR/$abi/libuniffi_offline_protocol.so"
  if [ -f "$lib_path" ]; then
    echo "✅ Android $abi library found"
    echo "   Size: $(du -h "$lib_path" | cut -f1)"
  else
    echo "❌ Android $abi library missing: $lib_path"
    MISSING_ABIS+=("$abi")
  fi
done

if [ ${#MISSING_ABIS[@]} -gt 0 ]; then
  echo ""
  echo "Missing Android ABIs: ${MISSING_ABIS[*]}"
  echo "Run: npm run build:android"
  exit 1
fi

echo ""

# Check TypeScript files
if [ ! -f "$RN_DIR/src/index.ts" ] || [ ! -f "$RN_DIR/src/types.ts" ]; then
  echo "❌ TypeScript source files missing"
  exit 1
fi

echo "✅ TypeScript source files found"
echo ""

# Check native module files
if [ ! -f "$RN_DIR/ios/OfflineProtocolModule.swift" ]; then
  echo "❌ iOS native module missing"
  exit 1
fi

if [ ! -f "$RN_DIR/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt" ]; then
  echo "❌ Android native module missing"
  exit 1
fi

# The podspec must be at the package root or iOS autolinking silently skips this
# dependency: React Native globs "*.podspec" in the package root only.
if [ ! -f "$RN_DIR/MeshSdk.podspec" ]; then
  echo "❌ MeshSdk.podspec missing from the package root"
  echo "   iOS autolinking cannot find a podspec under ios/ — it must stay at the root"
  exit 1
fi

echo "✅ Native module files found"
echo ""

# The shipped docs (README, docs/UPGRADING.md §12.1, the integration guides) and
# the runtime linking-error string in src/constants.ts all state that iOS
# autolinking landed in 0.20.0. Publishing this tree under anything earlier — a
# 0.19.x patch, say — would ship that claim against a release where it is false,
# and send consumers to a migration section for a version that never existed.
# The version is stamped from the git tag at release time, so this is the only
# place that can catch the mismatch. Pre-release suffixes are stripped, so
# 0.20.0-rc.1 satisfies it.
AUTOLINK_MIN_VERSION="0.20.0"
PKG_VERSION="$(node -p "require('$RN_DIR/package.json').version")"
PKG_CORE_VERSION="${PKG_VERSION%%-*}"
LOWEST="$(printf '%s\n%s\n' "$AUTOLINK_MIN_VERSION" "$PKG_CORE_VERSION" | sort -V | head -1)"
if [ "$LOWEST" != "$AUTOLINK_MIN_VERSION" ]; then
  echo "❌ Version $PKG_VERSION is below $AUTOLINK_MIN_VERSION"
  echo "   The docs and the runtime linking-error text state that iOS autolinking"
  echo "   and the XCFramework landed in $AUTOLINK_MIN_VERSION. Publishing below that"
  echo "   ships a false claim. Tag $AUTOLINK_MIN_VERSION or later, or update the"
  echo "   version stated in README.md, docs/UPGRADING.md, docs/react-native-integration.md,"
  echo "   examples/react-native-app/INTEGRATION_GUIDE.md and src/constants.ts."
  exit 1
fi
echo "✅ Version $PKG_VERSION is consistent with the documented autolinking release"
echo ""

# Every check above reads the working tree, which proves an artifact was built —
# not that it ships. package.json's files[] allowlist decides that, and for the
# iOS binary it is the *only* reason the artifact ships at all: ios/libs/ is
# gitignored. Drop or mistype an entry there (or add an .npmignore) and every
# check above stays green while npm publishes a package with no native binary —
# the same silently-wrong-packaging class as the podspec autolinking could not
# see. So assert the real packlist.
#
# Everything required here was already proven present on disk above, so a miss
# has exactly one cause: the file was excluded from the tarball.
#
# --ignore-scripts because `npm pack` runs `prepare` (tsc), which the publish
# flow ran moments ago; without it this recompiles the package for nothing.
echo "Verifying the npm packlist..."

REQUIRED_PACKED=(
  "MeshSdk.podspec"
  "$IOS_XCFRAMEWORK_REL/Info.plist"
  "$IOS_XCFRAMEWORK_REL/ios-arm64/liboffline_protocol_uniffi.a"
  "$IOS_XCFRAMEWORK_REL/ios-arm64_x86_64-simulator/liboffline_protocol_uniffi.a"
  "ios/Generated/offline_protocol.swift"
  "ios/Generated/offline_protocolFFI.h"
  "ios/Generated/offline_protocolFFI.modulemap"
  "ios/OfflineProtocolModule.swift"
  "android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt"
  "src/index.ts"
  "src/types.ts"
)
for abi in "${ANDROID_ABIS[@]}"; do
  REQUIRED_PACKED+=("android/src/main/jniLibs/$abi/libuniffi_offline_protocol.so")
done

MISSING_PACKED="$(cd "$RN_DIR" && npm pack --dry-run --ignore-scripts --json | node -e '
  let raw = "";
  process.stdin.on("data", (chunk) => (raw += chunk)).on("end", () => {
    const packed = new Set(JSON.parse(raw)[0].files.map((f) => f.path));
    for (const p of process.argv.slice(1)) {
      if (!packed.has(p)) console.log("   " + p);
    }
  });
' "${REQUIRED_PACKED[@]}")"

if [ -n "$MISSING_PACKED" ]; then
  echo "❌ Built on disk but excluded from the npm tarball:"
  echo "$MISSING_PACKED"
  echo ""
  echo "   Publishing this would ship a package missing the files above."
  echo "   Fix the files[] allowlist in package.json (or remove the .npmignore"
  echo "   entry) that drops them. Note ios/libs/ is gitignored, so files[] is"
  echo "   the only thing putting the native binary in the tarball."
  exit 1
fi

echo "✅ npm packlist ships every validated artifact"
echo ""

echo "========================================="
echo "✅ All required files are present!"
echo "========================================="
echo ""
echo "Package is ready for publishing:"
echo "  npm publish"
echo ""
echo "Package size estimate:"
TOTAL_SIZE=$(du -sh "$RN_DIR" | cut -f1)
echo "  Total: $TOTAL_SIZE"
echo ""

