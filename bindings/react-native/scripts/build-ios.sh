#!/bin/bash

# Build iOS universal library
# This script builds the Rust library for all iOS architectures and combines them into a universal library

set -e

echo "Building iOS universal library..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
# shellcheck source=shared/xcframework.sh
source "$SCRIPT_DIR/shared/xcframework.sh"
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

# Why an XCFramework, and why both slices share one archive basename: see
# scripts/shared/xcframework.sh.
package_xcframework \
  "$OUTPUT_DIR" \
  "$PROJECT_ROOT/target/aarch64-apple-ios/release/liboffline_protocol_uniffi.a" \
  "$PROJECT_ROOT/target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a" \
  "$PROJECT_ROOT/target/x86_64-apple-ios/release/liboffline_protocol_uniffi.a"

print_xcframework_slices "$XCFRAMEWORK"

echo ""
echo "✅ iOS build complete!"

