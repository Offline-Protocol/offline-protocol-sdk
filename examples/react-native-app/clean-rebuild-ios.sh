#!/bin/bash

# Deep clean and rebuild for iOS

set -e

echo "🧹 Performing deep clean..."

cd "$(dirname "$0")"

# Kill Metro
echo "Stopping Metro bundler..."
pkill -f "node.*metro" || true

# Clean React Native
echo "Cleaning React Native..."
rm -rf node_modules/.cache

# Clean iOS
echo "Cleaning iOS build artifacts..."
cd ios
rm -rf build
rm -rf Pods
rm -rf ~/Library/Developer/Xcode/DerivedData/OfflineProtocolExample-*

# Reinstall Pods
echo "Reinstalling CocoaPods..."
pod install

cd ..

echo ""
echo "✅ Deep clean complete!"
echo ""
echo "Now open Xcode and build there to see detailed errors:"
echo "  cd ios"
echo "  open OfflineProtocolExample.xcworkspace"
echo ""
echo "In Xcode:"
echo "  1. Product → Clean Build Folder (Cmd+Shift+K)"
echo "  2. Product → Build (Cmd+B)"
echo "  3. Check the Issue Navigator (Cmd+5) for detailed error"


