#!/bin/bash

# Build UniFFI libraries for all platforms and generate bindings
# This script orchestrates building UniFFI for iOS and Android

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "========================================="
echo "Building UniFFI libraries for all platforms"
echo "========================================="
echo ""

# Check if uniffi-bindgen is installed
if ! command -v uniffi-bindgen &> /dev/null; then
  echo "⚠️  WARNING: uniffi-bindgen not found!"
  echo ""
  echo "Native libraries will be built, but bindings won't be generated."
  echo "To install uniffi-bindgen, run:"
  echo "  cargo install uniffi --version 0.30.0 --features cli"
  echo ""
  echo "Press Enter to continue or Ctrl+C to cancel..."
  read -r
fi

# Build iOS
echo "📱 Building iOS + generating Swift bindings..."
bash "$SCRIPT_DIR/build-uniffi-ios.sh"

echo ""
echo "========================================="
echo ""

# Build Android
echo "🤖 Building Android + generating Kotlin bindings..."
bash "$SCRIPT_DIR/build-uniffi-android.sh"

echo ""
echo "========================================="
echo ""
echo "✅ All platforms built successfully!"
echo ""
echo "Pre-built libraries are ready for npm distribution:"
echo "  iOS Device:     bindings/react-native/ios/libs/liboffline_protocol_uniffi_device.a"
echo "  iOS Simulator:  bindings/react-native/ios/libs/liboffline_protocol_uniffi_sim.a"
echo "  Android:        bindings/react-native/android/src/main/jniLibs/**/*.so"
echo ""

if command -v uniffi-bindgen &> /dev/null; then
  echo "Generated bindings:"
  echo "  Swift:   bindings/react-native/ios/Generated/"
  echo "  Kotlin:  bindings/react-native/android/src/main/java/"
  echo ""
  echo "Next steps:"
  echo "  1. Update OfflineProtocolModule.swift to use generated Swift bindings"
  echo "  2. Update OfflineProtocolModule.kt to use generated Kotlin bindings"
  echo "  3. Test on iOS and Android"
  echo "  4. Run 'npm publish' to distribute the package"
else
  echo "⚠️  Bindings were NOT generated (uniffi-bindgen not installed)"
  echo ""
  echo "To generate bindings, install uniffi-bindgen and run:"
  echo "  cargo install uniffi --version 0.30.0 --features cli"
  echo "  npm run generate:bindings"
fi

echo ""

