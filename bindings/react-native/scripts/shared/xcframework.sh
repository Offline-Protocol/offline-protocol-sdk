#!/bin/bash
#
# Shared iOS XCFramework packaging. Sourced by build-uniffi-ios.sh, now the
# only caller: build-ios.sh used to source this too and duplicate the rest of
# the build, and is a thin wrapper over build-uniffi-ios.sh since that copy
# turned out to pair fresh native artifacts with stale committed bindings.
# build-uniffi-ios.sh is what the release workflow runs, so a change here
# reaches production — keep it dependency-free.
#
# WHY AN XCFRAMEWORK AND NOT LOOSE ARCHIVES
#
# Device and simulator arm64 cannot coexist in one `lipo` archive, so there are
# necessarily two slices. They go into an XCFramework rather than two loose `.a`
# files so that Xcode/CocoaPods select the slice per build SDK: a flat directory
# of archives makes CocoaPods emit an unconditional `-l` for every archive it
# finds, and the only xcconfig that could gate them is the *app* target's,
# somewhere a podspec has no way to reach. That is the defect that made
# simulator builds unlinkable before #312.
#
# WHY BOTH SLICES SHARE ONE ARCHIVE BASENAME
#
# CocoaPods derives a single `-l<name>` flag for the whole XCFramework and
# applies it to whichever slice it copied. Distinct names — the old
# _device/_sim suffixes, which existed only so both could sit in one flat
# directory — would leave that flag pointing at nothing on one of the two
# platforms. Hence ARCHIVE_BASENAME below is used for both.

ARCHIVE_BASENAME="liboffline_protocol_uniffi.a"

# package_xcframework <output_dir> <device_a> <sim_arm64_a> <sim_x86_64_a>
#
# Stages the device archive and a fat simulator archive under one shared
# basename, then builds <output_dir>/offline_protocol_uniffi.xcframework.
package_xcframework() {
  local output_dir="$1"
  local device_lib="$2"
  local sim_arm64_lib="$3"
  local sim_x86_64_lib="$4"

  local xcframework="$output_dir/offline_protocol_uniffi.xcframework"

  local stage
  stage="$(mktemp -d)"
  # The staging dir holds a full copy of both archives (hundreds of MB), so it
  # must not leak. $stage is expanded into the trap body *now*, at set time,
  # rather than left for the trap to expand: it is a `local`, and an EXIT trap
  # fires after this function has returned, by which point the name is out of
  # scope and the cleanup would silently become `rm -rf ""`. The trap covers
  # the failure path; the success path clears it and removes the dir directly.
  trap "rm -rf '$stage'" EXIT
  mkdir -p "$stage/device" "$stage/simulator"

  echo "Staging device slice..."
  cp "$device_lib" "$stage/device/$ARCHIVE_BASENAME"

  echo "Staging simulator slice (Intel + Apple Silicon)..."
  lipo -create "$sim_arm64_lib" "$sim_x86_64_lib" \
    -output "$stage/simulator/$ARCHIVE_BASENAME"

  # -create-xcframework refuses to write over an existing bundle.
  rm -rf "$xcframework"
  xcodebuild -create-xcframework \
    -library "$stage/device/$ARCHIVE_BASENAME" \
    -library "$stage/simulator/$ARCHIVE_BASENAME" \
    -output "$xcframework"

  # No -headers: the FFI header and modulemap stay in ios/Generated/ and reach
  # Swift via the podspec's SWIFT_INCLUDE_PATHS / HEADER_SEARCH_PATHS.

  # Remove the superseded loose archives so a stale Podfile cannot pick one up.
  rm -f "$output_dir/liboffline_protocol_uniffi_device.a" \
        "$output_dir/liboffline_protocol_uniffi_sim.a"

  rm -rf "$stage"
  trap - EXIT

  echo "iOS XCFramework created: $xcframework"
}

# print_xcframework_slices <xcframework>
print_xcframework_slices() {
  local xcframework="$1"

  echo ""
  echo "XCFramework slices:"
  for slice in "$xcframework"/*/; do
    echo "  $(basename "$slice"): $(lipo -info "$slice/$ARCHIVE_BASENAME" | sed 's/.*: //')"
  done
}
