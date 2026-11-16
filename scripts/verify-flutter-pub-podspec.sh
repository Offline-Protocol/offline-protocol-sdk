#!/usr/bin/env bash
#
# Verify offline_protocol.podspec evaluates for a pub-assembled Flutter tree.
# dart pub publish --dry-run does not load the iOS podspec; this does.
#
# Usage:
#   bash scripts/verify-flutter-pub-podspec.sh build/flutter-pub-package
#
# Requires CocoaPods (`pod ipc spec`) on PATH.

set -euo pipefail

ROOT="${1:?usage: verify-flutter-pub-podspec.sh <assembled-package-root>}"

die() {
  echo "ERROR: $*" >&2
  exit 1
}

IOS="$ROOT/ios"
PODSPEC="$IOS/offline_protocol.podspec"
META="$IOS/pub_bridge_version.json"

[ -d "$ROOT" ] || die "not a directory: $ROOT"
[ -f "$PODSPEC" ] || die "missing $PODSPEC"
[ -f "$META" ] || die "missing $META (not a pub layout?)"
[ -f "$IOS/BridgeRN/Generated/offline_protocol.swift" ] || die "missing vendored BridgeRN bindings"

ruby -rjson -e '
  meta = JSON.parse(File.read(ARGV[0]))
  %w[version ios_deployment_target].each do |k|
    abort "pub_bridge_version.json missing #{k}" if meta[k].nil? || meta[k].to_s.empty?
  end
' "$META"

command -v pod >/dev/null 2>&1 || die "CocoaPods (pod) is required; install cocoapods"

json="$(cd "$IOS" && pod ipc spec offline_protocol.podspec)"
[ -n "$json" ] || die "pod ipc spec produced no output"

ruby -rjson -e '
  spec = JSON.parse(STDIN.read)
  abort "spec name must be offline_protocol" unless spec["name"] == "offline_protocol"
  abort "spec version missing" if spec["version"].to_s.empty?
  ios = spec.dig("platforms", "ios")
  abort "spec ios platform missing" if ios.nil? || ios.to_s.empty?
' <<<"$json"

echo "Flutter pub podspec OK ($PODSPEC)"