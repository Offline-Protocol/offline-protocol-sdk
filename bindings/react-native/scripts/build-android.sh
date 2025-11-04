#!/usr/bin/env bash

# Build Android libraries for all ABIs
# This script builds the Rust library for all Android architectures
# Compatible with bash 3.2+ (macOS default)

set -e

echo "Building Android libraries..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/../android/src/main/jniLibs"

cd "$PROJECT_ROOT"

# Android ABIs and their Rust targets (bash 3.2 compatible)
ABIS=("arm64-v8a" "armeabi-v7a" "x86_64" "x86")
TARGETS=("aarch64-linux-android" "armv7-linux-androideabi" "x86_64-linux-android" "i686-linux-android")

# Check if NDK is available
if [ -z "$ANDROID_NDK_HOME" ] && [ -z "$NDK_HOME" ]; then
  echo "Warning: ANDROID_NDK_HOME or NDK_HOME not set."
  echo "Please set one of these environment variables to your Android NDK path."
  echo "Example: export ANDROID_NDK_HOME=/Users/\$USER/Library/Android/sdk/ndk/25.1.8937393"
  echo ""
  echo "Attempting to find NDK in common locations..."
  
  # Try to find NDK in common locations
  POSSIBLE_NDKS=(
    "$HOME/Library/Android/sdk/ndk"
    "$HOME/Android/Sdk/ndk"
    "/opt/android-ndk"
  )
  
  for ndk_path in "${POSSIBLE_NDKS[@]}"; do
    if [ -d "$ndk_path" ]; then
      # Use the first (usually latest) version found
      NDK_VERSION=$(ls -1 "$ndk_path" | sort -V | tail -1)
      if [ -n "$NDK_VERSION" ]; then
        export ANDROID_NDK_HOME="$ndk_path/$NDK_VERSION"
        echo "Found NDK at: $ANDROID_NDK_HOME"
        break
      fi
    fi
  done
  
  if [ -z "$ANDROID_NDK_HOME" ]; then
    echo "Error: Could not find Android NDK. Please install it or set ANDROID_NDK_HOME."
    exit 1
  fi
fi

# Add NDK toolchain to PATH
NDK_TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin"
if [ ! -d "$NDK_TOOLCHAIN" ]; then
  # Try alternate path for Apple Silicon
  NDK_TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-aarch64/bin"
fi

if [ -d "$NDK_TOOLCHAIN" ]; then
  export PATH="$NDK_TOOLCHAIN:$PATH"
  echo "Added NDK toolchain to PATH: $NDK_TOOLCHAIN"
else
  echo "Warning: Could not find NDK toolchain directory"
fi

# Ensure targets are installed
echo "Installing Android targets..."
for target in "${TARGETS[@]}"; do
  rustup target add "$target"
done

# Build for each ABI
echo "Building for Android ABIs..."
for i in "${!ABIS[@]}"; do
  abi="${ABIS[$i]}"
  target="${TARGETS[$i]}"
  echo "Building for $abi ($target)..."
  
  cargo build --release --target "$target" --package offline-protocol-ffi
  
  # Create output directory for this ABI
  mkdir -p "$OUTPUT_DIR/$abi"
  
  # Copy library to jniLibs
  cp "$PROJECT_ROOT/target/$target/release/liboffline_protocol_ffi.so" \
     "$OUTPUT_DIR/$abi/"
  
  echo "✅ Built and copied library for $abi"
done

echo ""
echo "Android libraries built and copied to $OUTPUT_DIR"
echo ""

# Print library info
echo "Library sizes:"
for abi in "${ABIS[@]}"; do
  lib_path="$OUTPUT_DIR/$abi/liboffline_protocol_ffi.so"
  if [ -f "$lib_path" ]; then
    size=$(du -h "$lib_path" | cut -f1)
    echo "  $abi: $size"
  fi
done

echo ""
echo "✅ Android build complete!"

