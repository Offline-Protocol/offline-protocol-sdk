#!/bin/bash

# Build all platforms
# This script orchestrates building for iOS and Android

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "========================================="
echo "Building Rust libraries for all platforms"
echo "========================================="
echo ""

# Build iOS
echo "📱 Building iOS..."
bash "$SCRIPT_DIR/build-ios.sh"

echo ""
echo "========================================="
echo ""

# Build Android
echo "🤖 Building Android..."
bash "$SCRIPT_DIR/build-android.sh"

echo ""
echo "========================================="
echo ""
echo "✅ All platforms built successfully!"
echo ""
echo "Pre-built libraries are ready for npm distribution:"
echo "  - iOS: bindings/react-native/ios/libs/offline_protocol_uniffi.xcframework (device + simulator slices)"
echo "  - Android: bindings/react-native/android/src/main/jniLibs/**/*.so"
echo ""
echo "You can now:"
echo "  1. Commit these binaries (or use git-lfs)"
echo "  2. Run 'npm publish' to distribute the package"
echo ""

