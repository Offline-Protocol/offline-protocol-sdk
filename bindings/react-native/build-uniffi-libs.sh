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

# Create universal library for iOS
echo "Creating universal iOS library..."
mkdir -p ../../bindings/react-native/ios/libs

lipo -create \
  ../../target/aarch64-apple-ios/release/liboffline_protocol_uniffi.a \
  ../../target/x86_64-apple-ios/release/liboffline_protocol_uniffi.a \
  -output ../../bindings/react-native/ios/libs/liboffline_protocol_uniffi_device.a

lipo -create \
  ../../target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a \
  ../../target/x86_64-apple-ios/release/liboffline_protocol_uniffi.a \
  -output ../../bindings/react-native/ios/libs/liboffline_protocol_uniffi_sim.a

# Create XCFramework (optional, for better Xcode integration)
# xcodebuild -create-xcframework \
#   -library ../../target/aarch64-apple-ios/release/liboffline_protocol_uniffi.a \
#   -library ../../target/aarch64-apple-ios-sim/release/liboffline_protocol_uniffi.a \
#   -output ../../bindings/react-native/ios/OfflineProtocolUniFFI.xcframework

# Android builds
echo "Building for Android..."
cargo build --release --target aarch64-linux-android
cargo build --release --target armv7-linux-androideabi
cargo build --release --target x86_64-linux-android
cargo build --release --target i686-linux-android

# Copy to Android jniLibs
echo "Copying Android libraries..."
mkdir -p ../../bindings/react-native/android/src/main/jniLibs/{arm64-v8a,armeabi-v7a,x86_64,x86}

cp ../../target/aarch64-linux-android/release/liboffline_protocol_uniffi.so \
   ../../bindings/react-native/android/src/main/jniLibs/arm64-v8a/

cp ../../target/armv7-linux-androideabi/release/liboffline_protocol_uniffi.so \
   ../../bindings/react-native/android/src/main/jniLibs/armeabi-v7a/

cp ../../target/x86_64-linux-android/release/liboffline_protocol_uniffi.so \
   ../../bindings/react-native/android/src/main/jniLibs/x86_64/

cp ../../target/i686-linux-android/release/liboffline_protocol_uniffi.so \
   ../../bindings/react-native/android/src/main/jniLibs/x86/

echo "✅ Native libraries built successfully!"
echo ""
echo "Next steps:"
echo "1. Generate Swift bindings: uniffi-bindgen generate --language swift ..."
echo "2. Generate Kotlin bindings: uniffi-bindgen generate --language kotlin ..."
echo "3. Update OfflineProtocolModule to use UniFFI bindings"
echo "4. Test on iOS and Android"

