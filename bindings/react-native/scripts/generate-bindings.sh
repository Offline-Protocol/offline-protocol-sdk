#!/bin/bash

# Generate UniFFI bindings only (without building native libraries)
# Useful when you just need to regenerate bindings from UDL changes

set -e

echo "Generating UniFFI bindings for Swift and Kotlin..."

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
UNIFFI_DIR="$PROJECT_ROOT/crates/offline-protocol-uniffi"
SWIFT_OUT="$SCRIPT_DIR/../ios/Generated"
KOTLIN_OUT="$SCRIPT_DIR/../android/src/main/java"

# Check if uniffi-bindgen is installed
if ! command -v uniffi-bindgen &> /dev/null; then
  echo "❌ Error: uniffi-bindgen not found!"
  echo ""
  echo "Install it with:"
  echo "  cargo install uniffi --version 0.30.0 --features cli"
  echo ""
  exit 1
fi

cd "$UNIFFI_DIR"

# Create output directories
mkdir -p "$SWIFT_OUT"
mkdir -p "$KOTLIN_OUT"

# Generate Swift bindings
echo "Generating Swift bindings..."
uniffi-bindgen generate \
  src/offline_protocol.udl \
  --language swift \
  --out-dir "$SWIFT_OUT"

echo "✅ Swift bindings generated in $SWIFT_OUT"

# Generate Kotlin bindings
echo "Generating Kotlin bindings..."
uniffi-bindgen generate \
  src/offline_protocol.udl \
  --language kotlin \
  --out-dir "$KOTLIN_OUT"

echo "✅ Kotlin bindings generated in $KOTLIN_OUT"

echo ""
echo "========================================="
echo "✅ All bindings generated successfully!"
echo "========================================="
echo ""
echo "Generated files:"
echo "  Swift:   $SWIFT_OUT"
echo "  Kotlin:  $KOTLIN_OUT"
echo ""
echo "Next steps:"
echo "  1. Review the generated bindings"
echo "  2. Update your Swift/Kotlin modules to use them"
echo "  3. Build and test your app"
echo ""

