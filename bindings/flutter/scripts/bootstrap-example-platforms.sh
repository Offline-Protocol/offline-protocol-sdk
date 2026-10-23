#!/usr/bin/env bash
# Materialize Flutter platform folders for the example app (Runner, Gradle wrapper).
#
# The example ships Dart and a minimal Android tree. The Xcode Runner project
# is generated locally because Flutter platform files are toolchain-versioned
# and are not committed.
#
# Usage (from repository root):
#   bindings/flutter/scripts/bootstrap-example-platforms.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXAMPLE_DIR="$(cd "$SCRIPT_DIR/../offline_protocol/example" && pwd)"

if ! command -v flutter >/dev/null 2>&1; then
  echo "Error: flutter not found on PATH." >&2
  echo "Install Flutter 3.10+ and retry." >&2
  exit 1
fi

cd "$EXAMPLE_DIR"
flutter pub get

# Regenerates missing platform folders without overwriting lib/ or pubspec.yaml.
flutter create . --platforms=android,ios --org com.offlineprotocol.flutter

echo "Example platforms ready under $EXAMPLE_DIR"
