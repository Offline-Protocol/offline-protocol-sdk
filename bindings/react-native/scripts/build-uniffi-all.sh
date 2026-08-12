#!/bin/bash

# Build UniFFI libraries for all platforms and generate bindings
# This script orchestrates building UniFFI for iOS and Android

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "========================================="
echo "Building UniFFI libraries for all platforms"
echo "========================================="
echo ""

# Check if uniffi-bindgen is installed.
#
# This used to warn and offer to continue, on the premise that the native
# libraries were still worth building on their own. They are not: the iOS and
# Android scripts below now fail rather than pair a fresh native library with
# the previously committed bindings, so continuing here only buys a longer wait
# before the same error. Fail immediately instead.
if ! command -v uniffi-bindgen &> /dev/null; then
  echo "❌ Error: uniffi-bindgen not found — the platform builds below would" >&2
  echo "   compile the native libraries and then fail at binding generation." >&2
  echo "" >&2
  echo "   cargo install uniffi --version 0.30.0 --features cli --locked" >&2
  echo "" >&2
  exit 1
fi

# Build iOS
echo "📱 Building iOS + generating bindings..."
bash "$SCRIPT_DIR/build-uniffi-ios.sh"

echo ""
echo "========================================="
echo ""

# Build Android
echo "🤖 Building Android + generating bindings..."
bash "$SCRIPT_DIR/build-uniffi-android.sh"

echo ""
echo "========================================="
echo ""
echo "✅ All platforms built successfully!"
echo ""
echo "Pre-built libraries are ready for npm distribution:"
echo "  iOS:            bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework (device + simulator slices)"
echo "  Android:        bindings/react-native/android/src/main/jniLibs/**/*.so"
echo ""

# Unconditional: the bindgen check at the top exits rather than continuing, and
# both platform scripts fail if generation does not happen, so reaching here
# means all three were written. The iOS and Android scripts each delegate to the
# shared generator, so the full set is written twice — deliberate, since both
# are also run on their own and idempotent regeneration is cheaper than letting
# either emit only its own language.
echo "Generated bindings (all three — one artifact set off one UDL):"
echo "  Swift:   bindings/react-native/ios/Generated/"
echo "  Kotlin:  bindings/react-native/android/src/main/java/uniffi/"
echo "  Python:  bindings/python/offline_protocol_sdk/"
echo ""
echo "Next steps:"
echo "  1. Commit all three together — a partial commit is drift"
echo "  2. Test on iOS and Android"
echo "  3. Run 'npm publish' to distribute the package"

echo ""

