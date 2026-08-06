#!/bin/bash

# Build UniFFI iOS library and generate Swift bindings
# This script builds the Rust UniFFI library for iOS and generates Swift bindings

set -e

echo "Building UniFFI iOS library and generating Swift bindings..."

# Navigate to the Rust project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
UNIFFI_DIR="$PROJECT_ROOT/crates/offline-protocol-uniffi"
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

# Device and simulator arm64 cannot coexist in one `lipo` archive, which is why
# this ships two slices. They go into an XCFramework rather than two loose `.a`
# files so that Xcode/CocoaPods select the slice per build SDK: a flat directory
# of archives forces every consumer to hand-write sdk-conditional linker flags,
# and those flags have to live on the *app* target to take effect, somewhere a
# podspec cannot reach.
#
# Both slices deliberately carry the SAME archive basename. CocoaPods derives a
# single `-l<name>` flag for the whole XCFramework and applies it to whichever
# slice it copied, so distinct names (the old _device/_sim suffixes, which
# existed only so both could sit in one flat directory) would leave that flag
# pointing at nothing for one of the two platforms.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/device" "$STAGE/simulator"

echo "Staging device slice..."
cp "$(static_lib_for aarch64-apple-ios)" \
   "$STAGE/device/liboffline_protocol_uniffi.a"

echo "Staging simulator slice (Intel + Apple Silicon)..."
lipo -create \
  "$(static_lib_for aarch64-apple-ios-sim)" \
  "$(static_lib_for x86_64-apple-ios)" \
  -output "$STAGE/simulator/liboffline_protocol_uniffi.a"

# -create-xcframework refuses to write over an existing bundle.
rm -rf "$XCFRAMEWORK"
xcodebuild -create-xcframework \
  -library "$STAGE/device/liboffline_protocol_uniffi.a" \
  -library "$STAGE/simulator/liboffline_protocol_uniffi.a" \
  -output "$XCFRAMEWORK"

# No -headers: the FFI header and modulemap stay in ios/Generated/ and reach
# Swift via the podspec's SWIFT_INCLUDE_PATHS / HEADER_SEARCH_PATHS, unchanged.

# Remove the superseded loose archives so a stale Podfile cannot pick one up.
rm -f "$OUTPUT_DIR/liboffline_protocol_uniffi_device.a" \
      "$OUTPUT_DIR/liboffline_protocol_uniffi_sim.a"

echo "iOS XCFramework created: $XCFRAMEWORK"

# Generate Swift bindings
echo ""
echo "Generating Swift bindings..."

if command -v uniffi-bindgen &> /dev/null; then
  cd "$UNIFFI_DIR"
  
  uniffi-bindgen generate \
    src/offline_protocol.udl \
    --language swift \
    --out-dir "$GENERATED_DIR"
  
  echo "✅ Swift bindings generated in $GENERATED_DIR"
else
  echo "⚠️  uniffi-bindgen not found!"
  echo "Install it with: cargo install uniffi --version 0.30.0 --features cli"
  echo ""
  echo "For now, the native libraries are built, but you'll need to"
  echo "generate Swift bindings manually when uniffi-bindgen is available."
  echo ""
  echo "To generate bindings later, run:"
  echo "  cd $UNIFFI_DIR"
  echo "  uniffi-bindgen generate src/offline_protocol.udl --language swift --out-dir $GENERATED_DIR"
fi

# Print slice info
echo ""
echo "XCFramework slices:"
for slice in "$XCFRAMEWORK"/*/; do
  echo "  $(basename "$slice"): $(lipo -info "$slice/liboffline_protocol_uniffi.a" | sed 's/.*: //')"
done

echo ""
echo "✅ iOS UniFFI build complete!"

