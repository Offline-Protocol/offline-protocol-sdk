#!/bin/bash
# Build script for UniFFI native libraries
# Run this after generating bindings with uniffi-bindgen

set -e

echo "Building UniFFI native libraries..."

# Navigate to uniffi crate
cd "$(dirname "$0")/../../crates/offline-protocol-uniffi"

# iOS builds
echo "Building for iOS..."
cargo build --release --target aarch64-apple-ios
cargo build --release --target x86_64-apple-ios # Simulator
cargo build --release --target aarch64-apple-ios-sim # M1 Simulator

# Cargo may put the staticlib in release/ or release/deps/
static_lib() {
  local arch=$1
  local root="../../target/$arch/release"
  if [[ -f "$root/liboffline_protocol_uniffi.a" ]]; then
    echo "$root/liboffline_protocol_uniffi.a"
  else
    echo "$root/deps/liboffline_protocol_uniffi.a"
  fi
}

# Create universal library for iOS
echo "Creating universal iOS library..."
mkdir -p ../../bindings/react-native/ios/libs

lipo -create \
  "$(static_lib aarch64-apple-ios)" \
  "$(static_lib x86_64-apple-ios)" \
  -output ../../bindings/react-native/ios/libs/liboffline_protocol_uniffi_device.a

lipo -create \
  "$(static_lib aarch64-apple-ios-sim)" \
  "$(static_lib x86_64-apple-ios)" \
  -output ../../bindings/react-native/ios/libs/liboffline_protocol_uniffi_sim.a

# Create XCFramework (optional, for better Xcode integration)
# xcodebuild -create-xcframework \
#   -library ../../target/aarch64-apple-ios/release/liboffline_protocol_uniffi.a \
#   -library ../../target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a \
#   -output ../../bindings/react-native/ios/OfflineProtocolUniFFI.xcframework

# Cargo may put the cdylib in release/ or release/deps/
so_lib() {
  local arch=$1
  local root="../../target/$arch/release"
  if [[ -f "$root/liboffline_protocol_uniffi.so" ]]; then
    echo "$root/liboffline_protocol_uniffi.so"
  else
    echo "$root/deps/liboffline_protocol_uniffi.so"
  fi
}

# Android builds
echo "Building for Android..."
cargo build --release --target aarch64-linux-android
cargo build --release --target armv7-linux-androideabi
cargo build --release --target x86_64-linux-android
cargo build --release --target i686-linux-android

# Copy to Android jniLibs
echo "Copying Android libraries..."
mkdir -p ../../bindings/react-native/android/src/main/jniLibs/{arm64-v8a,armeabi-v7a,x86_64,x86}

cp "$(so_lib aarch64-linux-android)" \
   ../../bindings/react-native/android/src/main/jniLibs/arm64-v8a/

cp "$(so_lib armv7-linux-androideabi)" \
   ../../bindings/react-native/android/src/main/jniLibs/armeabi-v7a/

cp "$(so_lib x86_64-linux-android)" \
   ../../bindings/react-native/android/src/main/jniLibs/x86_64/

cp "$(so_lib i686-linux-android)" \
   ../../bindings/react-native/android/src/main/jniLibs/x86/

echo "✅ Native libraries built successfully!"
echo ""
echo "Next steps:"
echo "1. Generate Swift bindings: uniffi-bindgen generate --language swift ..."
echo "2. Generate Kotlin bindings: uniffi-bindgen generate --language kotlin ..."
echo "3. Update OfflineProtocolModule to use UniFFI bindings"
echo "4. Test on iOS and Android"

