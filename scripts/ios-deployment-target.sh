#!/usr/bin/env bash

# Print the oldest iOS the SDK supports: the pod's deployment target, read
# out of the podspec.
#
# The podspec is where the number lives, and `ios_deployment_target` in
# bindings/react-native/scripts/shared/xcframework.sh is the one thing that
# reads it. The release build of the library calls that function directly.
# This script calls the same function for everything that is not a sourced
# shell script: the Swift package's manifest and the CI job that builds the
# library for it. A second parser here would be a second rule for one line,
# and the two would disagree on some podspec.
#
# Usage:
#   bash scripts/ios-deployment-target.sh
#
# Environment:
#   PACKAGE_SOURCE_ROOT  optional, defaults to the repository root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

# The function comes from this checkout, the podspec from ROOT: a fixture
# tree holds a podspec and not the build scripts.
# shellcheck source=../bindings/react-native/scripts/shared/xcframework.sh
source "$SCRIPT_DIR/../bindings/react-native/scripts/shared/xcframework.sh"

ios_deployment_target "$ROOT/bindings/react-native/MeshSdk.podspec"
