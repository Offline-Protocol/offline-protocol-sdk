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

# THE DEPLOYMENT TARGET
#
# No object in a slice declares a minimum OS newer than the one the pod
# declares.
#
# Two compilers read IPHONEOS_DEPLOYMENT_TARGET, and they fall back
# differently when it is unset. rustc falls back to the target's own floor,
# which is below anything the pod would declare. cc-rs, which every crate that
# compiles C or assembly builds through (today `ring` and `oslog`), falls
# back to the version of the installed SDK. So with the variable unset those
# objects were stamped for whichever Xcode built the release, and the linker
# of every application using the library warned about each of them. It is a
# warning today, and the same stamp is what the linker reads to decide an
# object cannot run on the application's oldest supported system.
#
# The number is the podspec's and is read from it, not written again here: a
# second copy is a number somebody raises in one place.

# ios_deployment_target <podspec>
#
# Prints the oldest iOS the pod admits. Fails when the podspec does not say,
# rather than let a build run with the variable empty, which cc-rs reads as
# unset.
ios_deployment_target() {
  local podspec="$1"
  local target

  target="$(sed -n 's/^[[:space:]]*s\.platforms[[:space:]]*=.*:ios[[:space:]]*=>[[:space:]]*"\([0-9][0-9.]*\)".*/\1/p' "$podspec" | head -1)"
  if [ -z "$target" ]; then
    echo "ERROR: $podspec does not declare s.platforms = { :ios => \"<version>\" }" >&2
    return 1
  fi
  echo "$target"
}

# newer_version <a> <b>
#
# Prints whichever of two versions is newer, comparing up to three components
# as numbers.
newer_version() {
  awk -v a="$1" -v b="$2" '
    BEGIN {
      split(a, x, "."); split(b, y, ".")
      for (i = 1; i <= 3; i++) {
        if (x[i] + 0 != y[i] + 0) {
          print (x[i] + 0 > y[i] + 0) ? a : b
          exit
        }
      }
      print a
    }
  '
}

# No arm64 simulator exists before iOS 14, so the toolchain raises that slice
# to 14.0 when the deployment target is below it.
IOS_ARM64_SIMULATOR_FLOOR="14.0"

# arm64_simulator_ceiling <deployment target>
#
# What the arm64 simulator slice may declare: the deployment target, or the
# simulator's floor where that is higher.
arm64_simulator_ceiling() {
  newer_version "$1" "$IOS_ARM64_SIMULATOR_FLOOR"
}

# min_os_offenders <ceiling>
#
# Reads `otool -l` output for an archive on stdin and prints one line for each
# object that declares a minimum OS above <ceiling>, or one that cannot be
# read as a version. Exits 0 when there are none, 1 when there are some, and
# 2 when it saw no stamp at all.
#
# Split from the otool call so the parsing can be tested on a machine that has
# no otool (scripts/tests/test-ios-min-os.sh at the repository root).
min_os_offenders() {
  local ceiling="$1"

  awk -v ceiling="$ceiling" '
    function above(version,    have, want, i) {
      split(version, have, ".")
      split(ceiling, want, ".")
      for (i = 1; i <= 3; i++) {
        if (have[i] + 0 != want[i] + 0) {
          return have[i] + 0 > want[i] + 0
        }
      }
      return 0
    }
    /\):$/ {
      member = $0
      # A fat archive names the architecture after the member.
      sub(/ \(architecture [^)]*\):$/, ":", member)
      sub(/\):$/, "", member)
      sub(/^.*\(/, "", member)
    }
    $1 == "cmd" { kind = $2 }
    (kind == "LC_BUILD_VERSION" && $1 == "minos") ||
    (kind == "LC_VERSION_MIN_IPHONEOS" && $1 == "version") {
      seen++
      if ($2 !~ /^[0-9]+(\.[0-9]+)*$/) {
        print member " declares a minimum OS that is not a version: " $2
        bad++
      } else if (above($2)) {
        print member " declares " $2
        bad++
      }
    }
    END {
      if (seen == 0) exit 2
      if (bad > 0) exit 1
      exit 0
    }
  '
}

# assert_archive_min_os <archive> <ceiling>
#
# An archive the gate could not read fails, the same as one with an object
# that is too new. That includes one otool read part of: it prints what it
# parsed and then exits non-zero, and the objects it never reached are the
# ones nothing looked at.
assert_archive_min_os() {
  local archive="$1"
  local ceiling="$2"
  local dump
  local offenders
  local status=0

  if ! dump="$(otool -l "$archive")"; then
    echo "ERROR: otool could not read all of $archive, so the check proves nothing." >&2
    return 1
  fi

  offenders="$(printf '%s\n' "$dump" | min_os_offenders "$ceiling")" || status=$?

  case "$status" in
    0)
      echo "  $archive: no object needs an iOS newer than $ceiling"
      ;;
    1)
      echo "ERROR: $archive holds objects built for a newer iOS than $ceiling:" >&2
      echo "$offenders" | head -5 | sed 's/^/  /' >&2
      echo "  ($(echo "$offenders" | wc -l | tr -d ' ') in total)" >&2
      echo "An application that supports iOS $ceiling links them with a warning for each." >&2
      echo "IPHONEOS_DEPLOYMENT_TARGET must reach every compiler in the build, the C" >&2
      echo "one included." >&2
      return 1
      ;;
    2)
      echo "ERROR: found no minimum OS stamp in $archive, so the check proves nothing." >&2
      return 1
      ;;
    *)
      echo "ERROR: could not parse the otool output for $archive (status $status)." >&2
      return 1
      ;;
  esac
}

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
  # shellcheck disable=SC2064
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
