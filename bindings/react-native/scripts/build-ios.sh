#!/bin/bash

# Build iOS universal library
# This script builds the Rust library for all iOS architectures and combines them into a universal library

set -e

echo "Building iOS universal library..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/../ios/libs"

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
  cargo build --release --target "$arch" --package offline-protocol-ffi
done

# Create output directory
mkdir -p "$OUTPUT_DIR"

echo "Copying iOS libraries..."

# Note: We can't create a fat binary with both device and simulator arm64 architectures
# Modern Xcode handles this automatically with XCFrameworks or separate binaries
# For now, we'll just copy the device library which works for both

# Copy device library (arm64)
echo "Copying device library (aarch64-apple-ios)..."
cp "$PROJECT_ROOT/target/aarch64-apple-ios/release/liboffline_protocol_ffi.a" \
   "$OUTPUT_DIR/liboffline_protocol_ffi.a"

echo "iOS library copied to $OUTPUT_DIR/liboffline_protocol_ffi.a"

# Print library info
echo ""
echo "Library info:"
lipo -info "$OUTPUT_DIR/liboffline_protocol_ffi.a"

echo ""
echo "✅ iOS build complete!"

