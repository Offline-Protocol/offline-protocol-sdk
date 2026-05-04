#!/bin/bash

# Prepare for npm publish
# Validates that all required binaries are present before publishing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RN_DIR="$SCRIPT_DIR/.."

echo "Validating pre-built binaries for npm package..."
echo ""

# Check iOS library
IOS_LIB="$RN_DIR/ios/libs/liboffline_protocol_uniffi_device.a"
IOS_SIM_LIB="$RN_DIR/ios/libs/liboffline_protocol_uniffi_sim.a"
if [ -f "$IOS_LIB" ]; then
  echo "✅ iOS library found: $IOS_LIB"
  echo "   Size: $(du -h "$IOS_LIB" | cut -f1)"
  echo "   Architectures: $(lipo -info "$IOS_LIB" | cut -d: -f3)"
else
  echo "❌ iOS library missing: $IOS_LIB"
  echo "   Run: npm run build:ios"
  exit 1
fi

echo ""

# Check Android libraries
ANDROID_DIR="$RN_DIR/android/src/main/jniLibs"
ANDROID_ABIS=("arm64-v8a" "armeabi-v7a" "x86_64" "x86")
MISSING_ABIS=()

for abi in "${ANDROID_ABIS[@]}"; do
  lib_path="$ANDROID_DIR/$abi/libuniffi_offline_protocol.so"
  if [ -f "$lib_path" ]; then
    echo "✅ Android $abi library found"
    echo "   Size: $(du -h "$lib_path" | cut -f1)"
  else
    echo "❌ Android $abi library missing: $lib_path"
    MISSING_ABIS+=("$abi")
  fi
done

if [ ${#MISSING_ABIS[@]} -gt 0 ]; then
  echo ""
  echo "Missing Android ABIs: ${MISSING_ABIS[*]}"
  echo "Run: npm run build:android"
  exit 1
fi

echo ""

# Check TypeScript files
if [ ! -f "$RN_DIR/src/index.ts" ] || [ ! -f "$RN_DIR/src/types.ts" ]; then
  echo "❌ TypeScript source files missing"
  exit 1
fi

echo "✅ TypeScript source files found"
echo ""

# Check native module files
if [ ! -f "$RN_DIR/ios/OfflineProtocolModule.swift" ]; then
  echo "❌ iOS native module missing"
  exit 1
fi

if [ ! -f "$RN_DIR/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt" ]; then
  echo "❌ Android native module missing"
  exit 1
fi

echo "✅ Native module files found"
echo ""
echo "========================================="
echo "✅ All required files are present!"
echo "========================================="
echo ""
echo "Package is ready for publishing:"
echo "  npm publish"
echo ""
echo "Package size estimate:"
TOTAL_SIZE=$(du -sh "$RN_DIR" | cut -f1)
echo "  Total: $TOTAL_SIZE"
echo ""

