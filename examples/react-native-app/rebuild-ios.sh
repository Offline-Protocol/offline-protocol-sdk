#!/bin/bash

# Rebuild iOS native modules with diagnostic logging
# This script rebuilds the iOS native modules after adding BLE diagnostic logging

set -e

echo "🔧 Rebuilding iOS native modules with BLE diagnostics..."

# Navigate to iOS directory
cd "$(dirname "$0")/ios"

echo "📦 Reinstalling CocoaPods..."
pod install

echo "🧹 Cleaning build folder..."
xcodebuild -workspace OfflineProtocolExample.xcworkspace \
  -scheme OfflineProtocolExample \
  -configuration Debug \
  clean

echo ""
echo "✅ iOS modules ready to rebuild"
echo ""
echo "Next steps:"
echo "1. Open Xcode: open OfflineProtocolExample.xcworkspace"
echo "2. Connect your physical iOS device"
echo "3. Select your device in Xcode"
echo "4. Build and run (Cmd+R)"
echo ""
echo "You should now see diagnostic messages in the console like:"
echo "  [BLE] Starting BLE operations..."
echo "  [BLE] Central Manager state: poweredOn"
echo "  [BLE] 🔍 Starting BLE scanning..."
echo "  [BLE] ✅ Advertising started successfully"
echo ""
echo "To test peer discovery, you need TWO physical iOS devices running this app."



