#!/bin/bash

# Build UniFFI iOS library and generate Swift bindings
# This script builds the Rust UniFFI library for iOS and generates Swift bindings

set -e

echo "Building UniFFI iOS library and generating Swift bindings..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
# shellcheck source=shared/xcframework.sh
source "$SCRIPT_DIR/shared/xcframework.sh"
OUTPUT_DIR="$SCRIPT_DIR/../ios/libs"
GENERATED_DIR="$SCRIPT_DIR/../ios/Generated"
XCFRAMEWORK="$OUTPUT_DIR/offline_protocol_uniffi.xcframework"

cd "$PROJECT_ROOT"

# iOS architectures
IOS_ARCHS=(
  "aarch64-apple-ios"           # iOS devices (ARM64)
  "aarch64-apple-ios-sim"       # iOS simulator on Apple Silicon
  "x86_64-apple-ios"            # iOS simulator on Intel
)

# Ensure targets are installed
echo "Installing iOS targets..."
for arch in "${IOS_ARCHS[@]}"; do
  rustup target add "$arch"
done

# Build for each architecture
echo "Building UniFFI library for iOS architectures..."
for arch in "${IOS_ARCHS[@]}"; do
  echo "Building for $arch..."
  cargo build --release --target "$arch" --package offline-protocol-uniffi
done

# Create output directories
mkdir -p "$OUTPUT_DIR"
mkdir -p "$GENERATED_DIR"

# Cargo may put the staticlib in release/ or release/deps/ depending on version
static_lib_for() {
  local arch=$1
  local root="$PROJECT_ROOT/target/$arch/release"
  if [[ -f "$root/liboffline_protocol_uniffi.a" ]]; then
    echo "$root/liboffline_protocol_uniffi.a"
  else
    echo "$root/deps/liboffline_protocol_uniffi.a"
  fi
}

echo "Packaging the XCFramework..."

# Why an XCFramework, and why both slices share one archive basename: see
# scripts/shared/xcframework.sh.
package_xcframework \
  "$OUTPUT_DIR" \
  "$(static_lib_for aarch64-apple-ios)" \
  "$(static_lib_for aarch64-apple-ios-sim)" \
  "$(static_lib_for x86_64-apple-ios)"

# Generate bindings — all languages, not just Swift: they are one artifact set
# off one UDL (see scripts/generate-bindings.sh). Still tolerant of a missing
# uniffi-bindgen, because the native libraries above are the point of this
# script and they are already built by here.
echo ""
echo "Generating bindings..."

if command -v uniffi-bindgen &> /dev/null; then
  bash "$PROJECT_ROOT/scripts/generate-bindings.sh"
else
  echo "⚠️  uniffi-bindgen not found!"
  echo "Install it with: cargo install uniffi --version 0.30.0 --features cli"
  echo ""
  echo "For now, the native libraries are built, but you'll need to"
  echo "generate bindings manually when uniffi-bindgen is available:"
  echo "  bash $PROJECT_ROOT/scripts/generate-bindings.sh"
fi

print_xcframework_slices "$XCFRAMEWORK"

echo ""
echo "✅ iOS UniFFI build complete!"

