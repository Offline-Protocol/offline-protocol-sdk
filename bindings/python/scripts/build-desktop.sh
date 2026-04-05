#!/usr/bin/env bash

# Build UniFFI desktop library and generate Python bindings.
# This script builds the Rust UniFFI cdylib for the current (or specified)
# platform and generates Python bindings via uniffi-bindgen.
#
# Usage:
#   ./build-desktop.sh                      # auto-detect current platform
#   ./build-desktop.sh --target aarch64-apple-darwin   # explicit target (CI)
#   ./build-desktop.sh --release             # release build (default)
#   ./build-desktop.sh --debug               # debug build

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
UNIFFI_DIR="$PROJECT_ROOT/crates/offline-protocol-uniffi"
PKG_DIR="$SCRIPT_DIR/../offline_protocol_sdk"

# Defaults
TARGET=""
PROFILE="release"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --debug)
            PROFILE="debug"
            shift
            ;;
        --release)
            PROFILE="release"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--target <triple>] [--debug|--release]"
            exit 1
            ;;
    esac
done

# Auto-detect platform if --target not given
if [[ -z "$TARGET" ]]; then
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin)
            case "$ARCH" in
                arm64)  TARGET="aarch64-apple-darwin" ;;
                x86_64) TARGET="x86_64-apple-darwin" ;;
                *)      echo "Unsupported macOS arch: $ARCH"; exit 1 ;;
            esac
            ;;
        Linux)
            case "$ARCH" in
                x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
                aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
                *)       echo "Unsupported Linux arch: $ARCH"; exit 1 ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            TARGET="x86_64-pc-windows-msvc"
            ;;
        *)
            echo "Unsupported OS: $OS"
            exit 1
            ;;
    esac

    echo "Auto-detected platform: $TARGET"
fi

# Determine library filename by platform
case "$TARGET" in
    *-apple-darwin)
        LIB_NAME="liboffline_protocol_uniffi.dylib"
        ;;
    *-linux-*)
        LIB_NAME="liboffline_protocol_uniffi.so"
        ;;
    *-windows-*)
        LIB_NAME="offline_protocol_uniffi.dll"
        ;;
    *)
        echo "Cannot determine library name for target: $TARGET"
        exit 1
        ;;
esac

echo "Building UniFFI desktop library for $TARGET ($PROFILE)..."

cd "$PROJECT_ROOT"

# Ensure target is installed
rustup target add "$TARGET"

# Build
CARGO_ARGS=(build --target "$TARGET" --package offline-protocol-uniffi)
if [[ "$PROFILE" == "release" ]]; then
    CARGO_ARGS+=(--release)
fi
cargo "${CARGO_ARGS[@]}"

# Locate the compiled library (check release/ then deps/ — same pattern as iOS script)
LIB_DIR="$PROJECT_ROOT/target/$TARGET/$PROFILE"

cdylib_path() {
    if [[ -f "$LIB_DIR/$LIB_NAME" ]]; then
        echo "$LIB_DIR/$LIB_NAME"
    elif [[ -f "$LIB_DIR/deps/$LIB_NAME" ]]; then
        echo "$LIB_DIR/deps/$LIB_NAME"
    else
        echo ""
    fi
}

CDYLIB="$(cdylib_path)"
if [[ -z "$CDYLIB" ]]; then
    echo "ERROR: $LIB_NAME not found in $LIB_DIR or $LIB_DIR/deps"
    ls -la "$LIB_DIR"/ "$LIB_DIR/deps"/ 2>/dev/null || true
    exit 1
fi

echo "Found library: $CDYLIB"

# Copy native library into package
mkdir -p "$PKG_DIR"
cp "$CDYLIB" "$PKG_DIR/$LIB_NAME"
echo "Copied $LIB_NAME -> $PKG_DIR/"

# Create a symlink with the name the generated Python bindings expect.
# UniFFI's UDL-mode Python codegen loads "libuniffi.{dylib,so,dll}" — create
# the appropriately named symlink pointing at the real library.
case "$TARGET" in
    *-apple-darwin)  SYMLINK_NAME="libuniffi.dylib" ;;
    *-linux-*)       SYMLINK_NAME="libuniffi.so" ;;
    *-windows-*)     SYMLINK_NAME="uniffi.dll" ;;
esac
(cd "$PKG_DIR" && ln -sf "$LIB_NAME" "$SYMLINK_NAME")
echo "Symlinked $SYMLINK_NAME -> $LIB_NAME"

# Generate Python bindings
echo ""
echo "Generating Python bindings..."

if command -v uniffi-bindgen &>/dev/null; then
    cd "$UNIFFI_DIR"
    uniffi-bindgen generate \
        src/offline_protocol.udl \
        --language python \
        --out-dir "$PKG_DIR"

    echo "Python bindings generated in $PKG_DIR/"
else
    echo "uniffi-bindgen not found!"
    echo "Install it with: cargo install uniffi --version 0.30.0 --features cli"
    echo ""
    echo "Native library is built. Generate bindings manually when uniffi-bindgen is available:"
    echo "  cd $UNIFFI_DIR"
    echo "  uniffi-bindgen generate src/offline_protocol.udl --language python --out-dir $PKG_DIR"
    exit 1
fi

echo ""
echo "Build complete!"
echo "  Native library: $PKG_DIR/$LIB_NAME"
echo "  Python bindings: $PKG_DIR/offline_protocol.py"
echo ""
echo "To install the package:"
echo "  cd $SCRIPT_DIR/.."
echo "  pip install -e ."
