#!/bin/bash

# Build script for Android Rust libraries
# This script compiles the Rust FFI crate for all Android architectures

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Go up from scripts -> react-native -> bindings -> root
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
FFI_CRATE="$PROJECT_ROOT/crates/offline-protocol-ffi"
OUTPUT_DIR="$SCRIPT_DIR/../android/src/main/jniLibs"
CPP_DIR="$SCRIPT_DIR/../android/src/main/cpp"

echo "Building Rust libraries for Android..."
echo "Project root: $PROJECT_ROOT"
echo "FFI crate: $FFI_CRATE"
echo "Output directory: $OUTPUT_DIR"

# Android target architectures
ARCHS=(
    "aarch64-linux-android"      # arm64-v8a
    "armv7-linux-androideabi"    # armeabi-v7a
    "x86_64-linux-android"       # x86_64
    "i686-linux-android"         # x86
)

# Create output directories
mkdir -p "$OUTPUT_DIR/arm64-v8a"
mkdir -p "$OUTPUT_DIR/armeabi-v7a"
mkdir -p "$OUTPUT_DIR/x86_64"
mkdir -p "$OUTPUT_DIR/x86"

# Detect Android NDK
if [ -z "$ANDROID_NDK_HOME" ]; then
    # Try common locations
    if [ -d "$HOME/Library/Android/sdk/ndk" ]; then
        NDK_DIR="$HOME/Library/Android/sdk/ndk"
        # Get the latest NDK version
        NDK_VERSION=$(ls -1 "$NDK_DIR" | sort -V | tail -1)
        ANDROID_NDK_HOME="$NDK_DIR/$NDK_VERSION"
    elif [ -d "$HOME/Android/Sdk/ndk" ]; then
        NDK_DIR="$HOME/Android/Sdk/ndk"
        NDK_VERSION=$(ls -1 "$NDK_DIR" | sort -V | tail -1)
        ANDROID_NDK_HOME="$NDK_DIR/$NDK_VERSION"
    elif [ -d "$ANDROID_HOME/ndk" ]; then
        NDK_DIR="$ANDROID_HOME/ndk"
        NDK_VERSION=$(ls -1 "$NDK_DIR" | sort -V | tail -1)
        ANDROID_NDK_HOME="$NDK_DIR/$NDK_VERSION"
    else
        echo "Error: ANDROID_NDK_HOME not set and could not find NDK automatically"
        echo ""
        echo "Please install the Android NDK:"
        echo "  1. Open Android Studio → Tools → SDK Manager → SDK Tools"
        echo "  2. Check 'NDK (Side by side)' and click Apply"
        echo ""
        echo "Then set ANDROID_NDK_HOME:"
        echo "  export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/<version>"
        echo ""
        echo "Or find your NDK version:"
        echo "  ls ~/Library/Android/sdk/ndk/"
        exit 1
    fi
fi

echo "Using Android NDK: $ANDROID_NDK_HOME"

# Detect NDK toolchain path (darwin-arm64 for Apple Silicon, darwin-x86_64 for Intel, or linux-x86_64)
if [[ "$OSTYPE" == "darwin"* ]]; then
    if [ -d "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-arm64" ]; then
        NDK_TOOLCHAIN="darwin-arm64"
    elif [ -d "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64" ]; then
        NDK_TOOLCHAIN="darwin-x86_64"
    else
        echo "Error: Could not find NDK toolchain in $ANDROID_NDK_HOME/toolchains/llvm/prebuilt/"
        exit 1
    fi
else
    if [ -d "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64" ]; then
        NDK_TOOLCHAIN="linux-x86_64"
    else
        echo "Error: Could not find NDK toolchain in $ANDROID_NDK_HOME/toolchains/llvm/prebuilt/"
        exit 1
    fi
fi

NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$NDK_TOOLCHAIN/bin"
echo "Using NDK toolchain: $NDK_BIN"

# Build for each architecture
for arch in "${ARCHS[@]}"; do
    echo ""
    echo "Building for $arch..."
    
    # Install target if not already installed
    rustup target add "$arch" || true
    
    # Set up linker environment variables
    case "$arch" in
        "aarch64-linux-android")
            export CC_aarch64_linux_android="$NDK_BIN/aarch64-linux-android21-clang"
            export AR_aarch64_linux_android="$NDK_BIN/llvm-ar"
            export CXX_aarch64_linux_android="$NDK_BIN/aarch64-linux-android21-clang++"
            LINKER="$NDK_BIN/aarch64-linux-android21-clang"
            ABI="arm64-v8a"
            ;;
        "armv7-linux-androideabi")
            export CC_armv7_linux_androideabi="$NDK_BIN/armv7a-linux-androideabi21-clang"
            export AR_armv7_linux_androideabi="$NDK_BIN/llvm-ar"
            export CXX_armv7_linux_androideabi="$NDK_BIN/armv7a-linux-androideabi21-clang++"
            LINKER="$NDK_BIN/armv7a-linux-androideabi21-clang"
            ABI="armeabi-v7a"
            ;;
        "x86_64-linux-android")
            export CC_x86_64_linux_android="$NDK_BIN/x86_64-linux-android21-clang"
            export AR_x86_64_linux_android="$NDK_BIN/llvm-ar"
            export CXX_x86_64_linux_android="$NDK_BIN/x86_64-linux-android21-clang++"
            LINKER="$NDK_BIN/x86_64-linux-android21-clang"
            ABI="x86_64"
            ;;
        "i686-linux-android")
            export CC_i686_linux_android="$NDK_BIN/i686-linux-android21-clang"
            export AR_i686_linux_android="$NDK_BIN/llvm-ar"
            export CXX_i686_linux_android="$NDK_BIN/i686-linux-android21-clang++"
            LINKER="$NDK_BIN/i686-linux-android21-clang"
            ABI="x86"
            ;;
    esac
    
    cd "$FFI_CRATE"
    
    # Build with explicit linker path
    RUSTFLAGS="-C linker=$LINKER" cargo build --release --target "$arch"
    
    # Copy library to output directory
    LIB_PATH="$PROJECT_ROOT/target/$arch/release/liboffline_protocol_ffi.so"
    if [ -f "$LIB_PATH" ]; then
        cp "$LIB_PATH" "$OUTPUT_DIR/$ABI/"
        echo "✓ Copied library to $OUTPUT_DIR/$ABI/liboffline_protocol_ffi.so"
    else
        echo "✗ Warning: Library not found at $LIB_PATH"
        exit 1
    fi
done

# Copy header file to cpp directory
HEADER_FILE="$FFI_CRATE/offline_protocol.h"
if [ -f "$HEADER_FILE" ]; then
    cp "$HEADER_FILE" "$CPP_DIR/"
    echo ""
    echo "✓ Copied header file to $CPP_DIR/offline_protocol.h"
else
    echo ""
    echo "✗ Warning: Header file not found at $HEADER_FILE"
fi

echo ""
echo "✓ Android build complete!"
echo "Libraries are in: $OUTPUT_DIR"
