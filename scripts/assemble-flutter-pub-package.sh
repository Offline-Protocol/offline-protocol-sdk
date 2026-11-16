#!/usr/bin/env bash
#
# Assemble a self-contained Flutter plugin tree for pub.dev.
# The in-repository plugin reaches shared bridge sources under
# bindings/react-native; a pub consumer has no such tree. This script copies
# the bridge, stages prebuilt UniFFI libraries, and writes the version pins the
# Gradle build and the podspec read, then verifies the layout a `pub publish`
# archive must carry.
#
# The package is the tracked files of the plugin and of the React Native bridge,
# plus the staged native libraries, and nothing else. A working tree also holds
# what a build or an IDE left there (local.properties, .idea/, .cxx/, .build/),
# hidden only by the .gitignore files the tree cannot ship; copied with the
# directory, it would publish to pub.dev, where a version is permanent.
#
# Two rules decide what pub puts in the archive, and both hide files silently:
#   - Inside a git work tree pub lists the package through git, so the
#     enclosing repository's .gitignore (build/, *.so, *.a) hides the pubspec,
#     the legal files and every native library. The output must be outside one.
#   - Pub honours a .gitignore inside the package even outside git, and the
#     plugin's ios/.gitignore ignores BridgeRN and Frameworks, which is the
#     whole iOS build. No .gitignore ships in the tree.
#
# Run after npm run build:uniffi:all (or after a release has downloaded the
# same artifacts into the React Native tree).
#
# Usage:
#   bash scripts/assemble-flutter-pub-package.sh \
#     --output "$RUNNER_TEMP/flutter-pub-package" --version 0.33.0
#
# Environment:
#   PACKAGE_SOURCE_ROOT  optional, defaults to the repository root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT="${PACKAGE_SOURCE_ROOT:-$REPO_ROOT}"
PLUGIN="$ROOT/bindings/flutter/offline_protocol"
STAGE="$REPO_ROOT/bindings/flutter/scripts/stage-plugin-native-artifacts.sh"
REPOSITORY_URL="https://github.com/Offline-Protocol/offline-protocol-sdk"

OUTPUT=""
VERSION=""

die() {
  echo "ERROR: $*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      [ "$#" -ge 2 ] || die "--output takes a path"
      OUTPUT="$2"
      shift 2
      ;;
    --version)
      [ "$#" -ge 2 ] || die "--version takes a value"
      VERSION="$2"
      shift 2
      ;;
    *)
      die "unknown argument: $1 (expected --output and --version)"
      ;;
  esac
done

[ -n "$OUTPUT" ] || die "--output is required"
[ -n "$VERSION" ] || die "--version is required"

# pub forbids leading v; release tags carry it.
VERSION="${VERSION#v}"

if [ -e "$OUTPUT" ]; then
  if [ -n "$(find "$OUTPUT" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    die "$OUTPUT exists and is not empty"
  fi
fi
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"

# See the header: pub would list the tree through git and lose most of it.
if command -v git >/dev/null 2>&1 &&
  git -C "$OUTPUT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  die "$OUTPUT is inside a git work tree, whose .gitignore pub would apply; assemble outside it (for example under \$RUNNER_TEMP)"
fi

# Copy the files git tracks under $1 into $2, less the pathspecs that follow,
# with their working-tree content. A tracked file deleted from the working tree
# fails the copy rather than leaving the package without it.
copy_tracked() {
  local src="$1" dest="$2"
  shift 2
  git -C "$src" rev-parse --is-inside-work-tree >/dev/null 2>&1 ||
    die "$src is not in a git work tree; the package is built from tracked files only"
  mkdir -p "$dest"
  git -C "$src" ls-files -z --cached -- . "$@" |
    tar -C "$src" --null -T - -cf - |
    tar -C "$dest" -xf -
}

stage_and_verify() {
  PACKAGE_SOURCE_ROOT="$ROOT" bash "$STAGE"
  PACKAGE_SOURCE_ROOT="$ROOT" bash "$STAGE" --check
}

assemble() {
  copy_tracked "$PLUGIN" "$OUTPUT" ':(exclude)example' ':(exclude)pubspec.lock'

  # The staged libraries under android/src/main/jniLibs and ios/Frameworks are
  # the only native copies the package ships, and the only untracked files it
  # takes. The plugin's Gradle build and podspec prefer them whenever they
  # hold the library, so the React Native tree's own copies would never be
  # read, and vendoring them would ship every native library twice and roughly
  # double the archive.
  mkdir -p "$OUTPUT/android/src/main/jniLibs" "$OUTPUT/ios/Frameworks"
  rsync -a "$PLUGIN/android/src/main/jniLibs/" "$OUTPUT/android/src/main/jniLibs/"
  rsync -a "$PLUGIN/ios/Frameworks/" "$OUTPUT/ios/Frameworks/"

  copy_tracked "$ROOT/bindings/react-native/android" "$OUTPUT/android/bridge-rn" \
    ':(exclude)src/main/jniLibs'

  printf '{"version":"%s"}\n' "$VERSION" >"$OUTPUT/android/pub_bridge_version.json"

  # The one parser for the deployment target (scripts/ios-deployment-target.sh),
  # which refuses a podspec that does not declare it exactly once.
  local ios_deployment
  ios_deployment="$(PACKAGE_SOURCE_ROOT="$ROOT" bash "$REPO_ROOT/scripts/ios-deployment-target.sh")"
  [ -n "$ios_deployment" ] || die "MeshSdk.podspec has no :ios deployment target"

  # The same copy `pod install` makes in the repository, with the same
  # exclude list, which a Rust guard holds to the podspec's, taken from the
  # tracked files only. The React Native package's libs/ is untracked, so it
  # stays out for the reason given above.
  local rn_ios
  rn_ios="$(mktemp -d)"
  copy_tracked "$ROOT/bindings/react-native/ios" "$rn_ios"
  BRIDGE_RN_SOURCE="$rn_ios" \
    BRIDGE_RN_DEST="$OUTPUT/ios/BridgeRN" \
    bash "$PLUGIN/ios/sync_bridge_rn.sh"
  rm -rf "$rn_ios"

  printf '{"version":"%s","ios_deployment_target":"%s"}\n' "$VERSION" "$ios_deployment" \
    >"$OUTPUT/ios/pub_bridge_version.json"

  for legal in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md EXPORT.md; do
    [ -f "$ROOT/$legal" ] || die "missing $ROOT/$legal"
    cp "$ROOT/$legal" "$OUTPUT/"
  done

  # pub.dev shows this file as the package's changelog and warns when it does
  # not name the version being published. The notes themselves live with the
  # release, so it points there rather than copying them.
  cat >"$OUTPUT/CHANGELOG.md" <<CHANGELOG
## $VERSION

Release notes: $REPOSITORY_URL/releases/tag/v$VERSION
CHANGELOG

  # See the header: every .gitignore copied in above would hide files from pub.
  find "$OUTPUT" -name .gitignore -type f -delete

  cat >"$OUTPUT/.pubignore" <<'PUBIGNORE'
# Maintainer paths only; the publish tree is not a git checkout.
.dart_tool/
.pub/
example/
PUBIGNORE

  grep -q "^version: " "$OUTPUT/pubspec.yaml" || die "pubspec.yaml has no version"
  perl -i -pe "s/^version: .*/version: $VERSION/" "$OUTPUT/pubspec.yaml"
  if ! grep -q '^publish_to:' "$OUTPUT/pubspec.yaml"; then
    perl -i -0pe 's/^environment:/publish_to: https:\/\/pub.dev\n\nenvironment:/m' "$OUTPUT/pubspec.yaml"
  fi

  verify_layout "$OUTPUT"
}

verify_layout() {
  local root="$1"
  local fail=0
  check() {
    if [ ! -e "$root/$1" ]; then
      echo "missing: $1" >&2
      fail=1
    fi
  }
  for abi in arm64-v8a armeabi-v7a x86 x86_64; do
    check "android/src/main/jniLibs/$abi/libuniffi_offline_protocol.so"
  done
  check "ios/Frameworks/offline_protocol_uniffi.xcframework/ios-arm64/liboffline_protocol_uniffi.a"
  check "ios/BridgeRN/Generated/offline_protocol.swift"
  check "android/bridge-rn/src/main/java/uniffi/offline_protocol/offline_protocol.kt"
  check "android/pub_bridge_version.json"
  check "ios/pub_bridge_version.json"
  check "pubspec.yaml"
  check "README.md"
  check "CHANGELOG.md"
  check "LICENSE"
  # One copy of each native library: see the vendoring step in assemble().
  for duplicate in android/bridge-rn/src/main/jniLibs ios/BridgeRN/libs; do
    if [ -e "$root/$duplicate" ]; then
      echo "duplicate native libraries: $duplicate" >&2
      fail=1
    fi
  done
  # Any ignore file but the root .pubignore hides files from pub.
  local ignore
  while IFS= read -r ignore; do
    echo "ignore file in the publish tree: ${ignore#"$root"/}" >&2
    fail=1
  done < <(find "$root" \( -name .gitignore -o -name .pubignore \) \
    -not -path "$root/.pubignore")
  [ "$fail" -eq 0 ] || die "assembled Flutter package failed layout checks"
}

stage_and_verify
assemble
echo "Assembled Flutter pub package at $OUTPUT (version $VERSION)"
