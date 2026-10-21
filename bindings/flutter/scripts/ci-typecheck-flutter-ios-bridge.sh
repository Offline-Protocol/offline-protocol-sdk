#!/usr/bin/env bash
# Typecheck the Flutter iOS plugin against the Flutter engine and shared RN Swift.
# Requires: Xcode, Flutter stable, arm64 UniFFI simulator archive under target/.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
RN_IOS="${REPO_ROOT}/bindings/react-native/ios"
FLUTTER_IOS="${REPO_ROOT}/bindings/flutter/offline_protocol/ios"
if [[ -z "${FLUTTER_ROOT:-}" || ! -d "${FLUTTER_ROOT}/bin/cache" ]]; then
  FLUTTER_BIN="$(command -v flutter)"
  FLUTTER_BIN="$(python3 -c "import os,sys; print(os.path.realpath(sys.argv[1]))" "$FLUTTER_BIN")"
  FLUTTER_ROOT="$(cd "$(dirname "$FLUTTER_BIN")/.." && pwd)"
fi
ENGINE_FRAMEWORK="${FLUTTER_ROOT}/bin/cache/artifacts/engine/ios/Flutter.xcframework/ios-arm64_x86_64-simulator"

if [[ ! -d "$ENGINE_FRAMEWORK" ]]; then
  echo "Flutter iOS engine framework not found at ${ENGINE_FRAMEWORK}" >&2
  echo "Run: flutter precache --ios" >&2
  exit 1
fi

DEPLOY="$(bash "${REPO_ROOT}/scripts/ios-deployment-target.sh")"

case "$(uname -m)" in
  arm64) SIM_TARGET="arm64-apple-ios${DEPLOY}-simulator"; ARCHIVE="${REPO_ROOT}/target/aarch64-apple-ios-sim/mobile/liboffline_protocol_uniffi.a" ;;
  x86_64) SIM_TARGET="x86_64-apple-ios${DEPLOY}-simulator"; ARCHIVE="${REPO_ROOT}/target/x86_64-apple-ios/mobile/liboffline_protocol_uniffi.a" ;;
  *) echo "unsupported runner arch $(uname -m)" >&2; exit 1 ;;
esac

if [[ ! -f "$ARCHIVE" ]]; then
  echo "missing UniFFI simulator archive at ${ARCHIVE}" >&2
  echo "Build with: cargo build --profile mobile --target <ios-sim> -p offline-protocol-uniffi" >&2
  exit 1
fi

bash "${FLUTTER_IOS}/sync_bridge_rn.sh"

SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"

BRIDGE_SWIFT=()
while IFS= read -r f; do BRIDGE_SWIFT+=("$f"); done < <(find "${FLUTTER_IOS}/BridgeRN" -name '*.swift' -type f | sort)
PLUGIN_SWIFT=()
while IFS= read -r f; do PLUGIN_SWIFT+=("$f"); done < <(find "${FLUTTER_IOS}/Classes" -name '*.swift' -type f | sort)

# Match the pod compile surface: BridgeRN/ only (sync_bridge_rn.sh), not RN_IOS
# paths in parallel — duplicate basenames make swiftc fail.
if ((${#BRIDGE_SWIFT[@]} == 0)); then
  echo "no Swift under ${FLUTTER_IOS}/BridgeRN; run sync_bridge_rn.sh first" >&2
  exit 1
fi

xcrun --sdk iphonesimulator swiftc -typecheck \
  -target "${SIM_TARGET}" \
  -sdk "$SDK" \
  -F "$ENGINE_FRAMEWORK" \
  -Xcc "-mios-simulator-version-min=${DEPLOY}" \
  -Xcc "-fmodule-map-file=${RN_IOS}/Generated/offline_protocolFFI.modulemap" \
  -Xcc "-I${RN_IOS}/Generated" \
  "${BRIDGE_SWIFT[@]}" \
  "${PLUGIN_SWIFT[@]}"

echo "OK: Flutter iOS bridge typecheck (${#PLUGIN_SWIFT[@]} plugin + ${#BRIDGE_SWIFT[@]} BridgeRN Swift files)"
