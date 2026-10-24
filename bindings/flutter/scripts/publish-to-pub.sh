#!/usr/bin/env bash
# Publish offline_protocol to pub.dev from a release tag checkout.
# Prerequisites: dart/flutter on PATH, pub.dev credentials (dart pub token add),
# native artifacts staged (release.yml runs stage-plugin-native-artifacts.sh).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN="${REPO_ROOT}/bindings/flutter/offline_protocol"

if [[ ! -d "${PLUGIN}/android/src/main/jniLibs" ]] && [[ ! -d "${PLUGIN}/ios/Frameworks" ]]; then
  echo "Native artifacts missing. From repo root run:" >&2
  echo "  bash bindings/flutter/scripts/stage-plugin-native-artifacts.sh" >&2
  exit 1
fi

cd "${PLUGIN}"
dart pub publish --dry-run
echo "Dry run OK. Publish with: dart pub publish"
