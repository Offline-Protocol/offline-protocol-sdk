#!/bin/bash

# Build iOS universal library
# This script builds the Rust library for all iOS architectures and combines them into a universal library

set -e

echo "Building iOS universal library..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/../ios/libs"
XCFRAMEWORK="$OUTPUT_DIR/offline_protocol_uniffi.xcframework"

cd "$PROJECT_ROOT"

# iOS architectures
IOS_ARCHS=(
  "aarch64-apple-ios"           # iOS devices (ARM64)
  "aarch64-apple-ios-sim"       # iOS simulator on Apple Silicon
  "x86_64-apple-ios"           # iOS simulator on Intel
)

# Ensure targets are installed
echo "Installing iOS targets..."
for arch in "${IOS_ARCHS[@]}"; do
  rustup target add "$arch"
done

# Build for each architecture
echo "Building for iOS architectures..."
for arch in "${IOS_ARCHS[@]}"; do
  echo "Building for $arch..."
  cargo build --release --target "$arch" --package offline-protocol-uniffi
done

# Create output directory
mkdir -p "$OUTPUT_DIR"

echo "Packaging the XCFramework..."

# Device and simulator arm64 cannot coexist in one `lipo` archive, so this ships
# two slices inside an XCFramework and lets Xcode/CocoaPods pick per build SDK.
# Both slices deliberately carry the SAME archive basename — CocoaPods derives a
# single `-l<name>` flag for the whole bundle and applies it to whichever slice
# it copied. See scripts/build-uniffi-ios.sh for the long-form rationale.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/device" "$STAGE/simulator"

echo "Staging device slice (aarch64-apple-ios)..."
cp "$PROJECT_ROOT/target/aarch64-apple-ios/release/liboffline_protocol_uniffi.a" \
   "$STAGE/device/liboffline_protocol_uniffi.a"

echo "Staging simulator slice (Intel + Apple Silicon)..."
lipo -create \
  "$PROJECT_ROOT/target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a" \
  "$PROJECT_ROOT/target/x86_64-apple-ios/release/liboffline_protocol_uniffi.a" \
  -output "$STAGE/simulator/liboffline_protocol_uniffi.a"

# -create-xcframework refuses to write over an existing bundle.
rm -rf "$XCFRAMEWORK"
xcodebuild -create-xcframework \
  -library "$STAGE/device/liboffline_protocol_uniffi.a" \
  -library "$STAGE/simulator/liboffline_protocol_uniffi.a" \
  -output "$XCFRAMEWORK"

# Remove the superseded loose archives so a stale Podfile cannot pick one up.
rm -f "$OUTPUT_DIR/liboffline_protocol_uniffi_device.a" \
      "$OUTPUT_DIR/liboffline_protocol_uniffi_sim.a"

echo "iOS XCFramework created: $XCFRAMEWORK"

# Print slice info
echo ""
echo "XCFramework slices:"
for slice in "$XCFRAMEWORK"/*/; do
  echo "  $(basename "$slice"): $(lipo -info "$slice/liboffline_protocol_uniffi.a" | sed 's/.*: //')"
done

echo ""
echo "✅ iOS build complete!"

