#!/usr/bin/env bash

# Build UniFFI Android libraries and generate Kotlin bindings
# This script builds the Rust UniFFI library for Android and generates Kotlin bindings
# Compatible with bash 3.2+ (macOS default)
#
# NOTE: CI release workflow (.github/workflows/release.yml) uses inline build
# logic with a matrix strategy instead of this script. If you change build
# targets, linker settings, or output paths here, update release.yml too.

set -e

echo "Building UniFFI Android libraries and generating Kotlin bindings..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
UNIFFI_DIR="$PROJECT_ROOT/crates/offline-protocol-uniffi"
OUTPUT_DIR="$SCRIPT_DIR/../android/src/main/jniLibs"
GENERATED_DIR="$SCRIPT_DIR/../android/src/main/java"

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

# Resolve NDK LLVM toolchain bin directory (supports modern NDK layouts).
LLVM_PREBUILT_ROOT="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt"
if [ ! -d "$LLVM_PREBUILT_ROOT" ]; then
  echo "Error: LLVM prebuilt toolchain root not found: $LLVM_PREBUILT_ROOT"
  exit 1
fi

OS_NAME="$(uname -s)"
ARCH_NAME="$(uname -m)"
TOOLCHAIN_CANDIDATES=()
if [ "$OS_NAME" = "Darwin" ]; then
  if [ "$ARCH_NAME" = "arm64" ]; then
    TOOLCHAIN_CANDIDATES=(
      "$LLVM_PREBUILT_ROOT/darwin-arm64/bin"
      "$LLVM_PREBUILT_ROOT/darwin-aarch64/bin"
      "$LLVM_PREBUILT_ROOT/darwin-x86_64/bin"
    )
  else
    TOOLCHAIN_CANDIDATES=(
      "$LLVM_PREBUILT_ROOT/darwin-x86_64/bin"
      "$LLVM_PREBUILT_ROOT/darwin-arm64/bin"
      "$LLVM_PREBUILT_ROOT/darwin-aarch64/bin"
    )
  fi
elif [ "$OS_NAME" = "Linux" ]; then
  TOOLCHAIN_CANDIDATES=(
    "$LLVM_PREBUILT_ROOT/linux-x86_64/bin"
    "$LLVM_PREBUILT_ROOT/linux-aarch64/bin"
  )
else
  echo "Error: Unsupported OS for Android NDK toolchain resolution: $OS_NAME"
  exit 1
fi

NDK_TOOLCHAIN=""
for candidate in "${TOOLCHAIN_CANDIDATES[@]}"; do
  if [ -d "$candidate" ]; then
    NDK_TOOLCHAIN="$candidate"
    break
  fi
done

if [ -z "$NDK_TOOLCHAIN" ]; then
  echo "Error: Could not find NDK toolchain bin directory under $LLVM_PREBUILT_ROOT"
  echo "Checked:"
  for candidate in "${TOOLCHAIN_CANDIDATES[@]}"; do
    echo "  - $candidate"
  done
  exit 1
fi

export PATH="$NDK_TOOLCHAIN:$PATH"
echo "Using NDK toolchain: $NDK_TOOLCHAIN"

# Configure Rust/Cargo Android linkers explicitly to avoid missing-linker errors.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_TOOLCHAIN/aarch64-linux-android21-clang"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$NDK_TOOLCHAIN/armv7a-linux-androideabi21-clang"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$NDK_TOOLCHAIN/x86_64-linux-android21-clang"
export CARGO_TARGET_I686_LINUX_ANDROID_LINKER="$NDK_TOOLCHAIN/i686-linux-android21-clang"
export AR="$NDK_TOOLCHAIN/llvm-ar"

for linker in \
  "$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER" \
  "$CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER" \
  "$CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER" \
  "$CARGO_TARGET_I686_LINUX_ANDROID_LINKER"; do
  if [ ! -x "$linker" ]; then
    echo "Error: required Android linker not found or not executable: $linker"
    exit 1
  fi
done

# Ensure targets are installed
echo "Installing Android targets..."
for target in "${TARGETS[@]}"; do
  rustup target add "$target"
done

# Build for each ABI
echo "Building UniFFI library for Android ABIs..."
for i in "${!ABIS[@]}"; do
  abi="${ABIS[$i]}"
  target="${TARGETS[$i]}"
  echo "Building for $abi ($target)..."
  
  cargo build --release --target "$target" --package offline-protocol-uniffi
  
  # Create output directory for this ABI
  mkdir -p "$OUTPUT_DIR/$abi"
  
  # Cargo may put the cdylib in release/ or release/deps/
  so_root="$PROJECT_ROOT/target/$target/release"
  if [ -f "$so_root/liboffline_protocol_uniffi.so" ]; then
    so_path="$so_root/liboffline_protocol_uniffi.so"
  else
    so_path="$so_root/deps/liboffline_protocol_uniffi.so"
  fi
  
  # Copy library to jniLibs
  # UniFFI generates code that looks for "uniffi_offline_protocol" which JNA converts to "libuniffi_offline_protocol.so"
  cp "$so_path" "$OUTPUT_DIR/$abi/libuniffi_offline_protocol.so"

  # Google Play requires 16 KB page-size alignment (flags live in
  # .cargo/config.toml); fail here rather than package a 4 KB-aligned lib.
  python3 "$SCRIPT_DIR/check-elf-alignment.py" "$OUTPUT_DIR/$abi/libuniffi_offline_protocol.so"

  echo "✅ Built and copied library for $abi"
done

echo ""
echo "Android libraries built and copied to $OUTPUT_DIR"

# Print library info
echo ""
echo "Library sizes:"
for abi in "${ABIS[@]}"; do
  lib_path="$OUTPUT_DIR/$abi/libuniffi_offline_protocol.so"
  if [ -f "$lib_path" ]; then
    size=$(du -h "$lib_path" | cut -f1)
    echo "  $abi: $size"
  fi
done

# Generate Kotlin bindings
echo ""
echo "Generating Kotlin bindings..."

if command -v uniffi-bindgen &> /dev/null; then
  mkdir -p "$GENERATED_DIR"
  cd "$UNIFFI_DIR"
  
  uniffi-bindgen generate \
    src/offline_protocol.udl \
    --language kotlin \
    --out-dir "$GENERATED_DIR"
  
  echo "✅ Kotlin bindings generated in $GENERATED_DIR"
else
  echo "⚠️  uniffi-bindgen not found!"
  echo "Install it with: cargo install uniffi --version 0.30.0 --features cli"
  echo ""
  echo "For now, the native libraries are built, but you'll need to"
  echo "generate Kotlin bindings manually when uniffi-bindgen is available."
  echo ""
  echo "To generate bindings later, run:"
  echo "  cd $UNIFFI_DIR"
  echo "  uniffi-bindgen generate src/offline_protocol.udl --language kotlin --out-dir $GENERATED_DIR"
fi

echo ""
echo "✅ Android UniFFI build complete!"

