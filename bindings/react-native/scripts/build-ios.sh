#!/bin/bash

# Build script for iOS Rust libraries
# This script compiles the Rust FFI crate for all iOS architectures and creates a universal framework

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Go up from scripts -> react-native -> bindings -> root
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
FFI_CRATE="$PROJECT_ROOT/crates/offline-protocol-ffi"
OUTPUT_DIR="$SCRIPT_DIR/../ios"

echo "Building Rust libraries for iOS..."
echo "Project root: $PROJECT_ROOT"
echo "FFI crate: $FFI_CRATE"
echo "Output directory: $OUTPUT_DIR"

# iOS target architectures
# Note: aarch64-apple-ios-sim works for both device and simulator on Apple Silicon
# We only include x86_64-apple-ios for Intel simulators
ARCHS=(
    "aarch64-apple-ios-sim"      # iOS device and simulator (Apple Silicon)
    "x86_64-apple-ios"           # iOS simulator (Intel x86_64)
)

# Create temporary directory for individual architecture builds
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo ""
echo "Building for each architecture..."

# Build for each architecture
for arch in "${ARCHS[@]}"; do
    echo ""
    echo "Building for $arch..."
    
    # Install target if not already installed
    rustup target add "$arch" || true
    
    cd "$FFI_CRATE"
    cargo build --release --target "$arch"
    
    # Copy library to temp directory
    LIB_PATH="$PROJECT_ROOT/target/$arch/release/liboffline_protocol_ffi.a"
    if [ -f "$LIB_PATH" ]; then
        cp "$LIB_PATH" "$TEMP_DIR/liboffline_protocol_ffi_${arch}.a"
        echo "✓ Built library for $arch"
    else
        echo "✗ Warning: Library not found at $LIB_PATH"
        exit 1
    fi
done

# Create universal library using lipo
echo ""
echo "Creating universal library..."

# Collect all architecture-specific libraries
LIBS=()
for arch in "${ARCHS[@]}"; do
    LIB_FILE="$TEMP_DIR/liboffline_protocol_ffi_${arch}.a"
    if [ -f "$LIB_FILE" ]; then
        LIBS+=("$LIB_FILE")
    fi
done

if [ ${#LIBS[@]} -eq 0 ]; then
    echo "✗ Error: No libraries found to combine"
    exit 1
fi

# Create universal library
echo "Combining ${#LIBS[@]} architectures..."
lipo -create "${LIBS[@]}" -output "$OUTPUT_DIR/liboffline_protocol_ffi.a"

if [ -f "$OUTPUT_DIR/liboffline_protocol_ffi.a" ]; then
    echo "✓ Created universal library: $OUTPUT_DIR/liboffline_protocol_ffi.a"
    
    # Show library info
    echo ""
    echo "Library architectures:"
    lipo -info "$OUTPUT_DIR/liboffline_protocol_ffi.a"
else
    echo "✗ Error: Failed to create universal library"
    exit 1
fi

# Copy header file
HEADER_FILE="$FFI_CRATE/offline_protocol.h"
if [ -f "$HEADER_FILE" ]; then
    cp "$HEADER_FILE" "$OUTPUT_DIR/"
    echo ""
    echo "✓ Copied header file to $OUTPUT_DIR/offline_protocol.h"
else
    echo ""
    echo "✗ Warning: Header file not found at $HEADER_FILE"
    exit 1
fi

echo ""
echo "✓ iOS build complete!"
echo "Universal library: $OUTPUT_DIR/liboffline_protocol_ffi.a"
echo "Header file: $OUTPUT_DIR/offline_protocol.h"
