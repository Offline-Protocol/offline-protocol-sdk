#!/usr/bin/env bash

# Print the url a consumer of the Swift package downloads the XCFramework from.
#
# The release writes it into the package manifest, the packaging script holds
# the manifest to it, and the distribution repository's workflow checks it
# before tagging. One place, so the three cannot disagree on where the archive
# is. It is a GitHub release asset of this repository, and GITHUB_REPOSITORY
# (set by Actions) names a fork's own release when a fork runs a release.
#
# Usage:
#   bash scripts/swiftpm-archive-url.sh <version>
#   bash scripts/swiftpm-archive-url.sh --asset-name <version>   # the file name only

set -euo pipefail

ASSET_ONLY=0
if [ "${1:-}" = --asset-name ]; then
  ASSET_ONLY=1
  shift
fi
[ "$#" -eq 1 ] && [ -n "$1" ] || {
  echo "usage: $0 [--asset-name] <version>" >&2
  exit 2
}
VERSION="$1"

ASSET="offline-protocol-$VERSION-swiftpm-xcframework.zip"
if [ "$ASSET_ONLY" = 1 ]; then
  echo "$ASSET"
else
  echo "https://github.com/${GITHUB_REPOSITORY:-Offline-Protocol/offline-protocol-sdk}/releases/download/v$VERSION/$ASSET"
fi
