#!/usr/bin/env bash
# Copy shared React Native iOS bridge sources into BridgeRN/ for CocoaPods.
# PathList only indexes files under the pod root; see offline_protocol.podspec.
#
# scripts/assemble-flutter-pub-package.sh runs this same copy into the pub.dev
# tree, so the exclude list below has one home. It sets:
#   BRIDGE_RN_SOURCE  the React Native ios/ directory to copy from
#   BRIDGE_RN_DEST    the BridgeRN directory to write
set -euo pipefail

IOS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RN_IOS="${BRIDGE_RN_SOURCE:-$(cd "$IOS_DIR/../../../react-native/ios" && pwd)}"
DEST="${BRIDGE_RN_DEST:-$IOS_DIR/BridgeRN}"

rm -rf "$DEST"
mkdir -p "$DEST"
rsync -a \
  --exclude OfflineProtocolModule.swift \
  --exclude OfflineProtocolModule.m \
  --exclude ProtocolEngineHostBridgeReactNative.swift \
  --exclude tests/ \
  --exclude Package.swift \
  "$RN_IOS/" "$DEST/"
