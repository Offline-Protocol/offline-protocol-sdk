#!/usr/bin/env bash
# Copy shared React Native iOS bridge sources into BridgeRN/ for CocoaPods.
# PathList only indexes files under the pod root; see offline_protocol.podspec.
set -euo pipefail

IOS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RN_IOS="$(cd "$IOS_DIR/../../../react-native/ios" && pwd)"
DEST="$IOS_DIR/BridgeRN"

rm -rf "$DEST"
mkdir -p "$DEST"
rsync -a \
  --exclude OfflineProtocolModule.swift \
  --exclude OfflineProtocolModule.m \
  --exclude InboundEventBuffer.swift \
  --exclude tests/ \
  --exclude Package.swift \
  "$RN_IOS/" "$DEST/"
