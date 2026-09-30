#!/usr/bin/env bash

# Drives the iOS deployment-target gate in
# bindings/react-native/scripts/shared/xcframework.sh without a Mac.
#
# The gate reads a real archive with otool, so it runs only on a Mac that
# built the iOS library: a release, and the Swift Package job on every pull
# request. Everything that can go wrong in it is text handling: reading the
# number out of the podspec, parsing what otool prints, and deciding what a
# failed otool means. This feeds it that text, with a stand-in for otool
# where one is called, on any runner.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RN_SCRIPTS="$REPO_ROOT/bindings/react-native/scripts"

# shellcheck source=../../bindings/react-native/scripts/shared/xcframework.sh
source "$RN_SCRIPTS/shared/xcframework.sh"

FAILURES=0
ASSERTIONS=0
# A test that stops asserting passes. Raise this with every check added.
EXPECTED_ASSERTIONS=69

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass() {
  echo "  ok: $*"
  ASSERTIONS=$((ASSERTIONS + 1))
}

fail() {
  echo "  FAIL: $*" >&2
  ASSERTIONS=$((ASSERTIONS + 1))
  FAILURES=$((FAILURES + 1))
}

# equal <name> <got> <want>
equal() {
  if [ "$2" = "$3" ]; then
    pass "$1"
  else
    fail "$1: got '$2', wanted '$3'"
  fi
}

# object <member> <load command> <field> <value>
#
# One archive member the way otool prints it: the member line, a command that
# carries no version, then the one under test and the `sdk` line after it.
object() {
  cat <<OBJECT
lib.a($1):
Load command 0
      cmd LC_SEGMENT_64
  cmdsize 392
Load command 1
      cmd $2
  cmdsize 24
    $3 $4
      sdk 26.5
OBJECT
}

# expect <name> <ceiling> <want status> <want output> <input>
expect() {
  local name="$1" ceiling="$2" want_status="$3" want_output="$4" input="$5"
  local output status=0

  output="$(printf '%s\n' "$input" | min_os_offenders "$ceiling")" || status=$?

  equal "$name: exit status" "$status" "$want_status"
  equal "$name: report" "$output" "$want_output"
}

echo "the parser"

expect "an object at the ceiling" 13.0 0 "" \
  "$(object a.o LC_BUILD_VERSION minos 13.0)"

expect "an object below the ceiling, old load command" 13.0 0 "" \
  "$(object a.o LC_VERSION_MIN_IPHONEOS version 10.0)"

expect "an old load command above the ceiling" 13.0 1 "a.o declares 14.0" \
  "$(object a.o LC_VERSION_MIN_IPHONEOS version 14.0)"

# The defect the gate exists for: C objects stamped for the SDK, among Rust
# objects that are fine. Every one of them is reported.
expect "objects above the ceiling" 13.0 1 "ring.o declares 26.5
oslog.o declares 26.5" \
  "$(object rust.o LC_BUILD_VERSION minos 13.0
     object ring.o LC_BUILD_VERSION minos 26.5
     object oslog.o LC_BUILD_VERSION minos 26.5)"

expect "a minor version above the ceiling" 13.0 1 "a.o declares 13.1" \
  "$(object a.o LC_BUILD_VERSION minos 13.1)"

expect "a patch version above the ceiling" 13.4.1 1 "a.o declares 13.4.2" \
  "$(object a.o LC_BUILD_VERSION minos 13.4.2)"

# 9.0 sorts after 13.0 as text, and 13.10 before 13.9 as a decimal.
expect "a major compares as a number" 13.0 0 "" \
  "$(object a.o LC_VERSION_MIN_IPHONEOS version 9.0)"
expect "a minor compares as a number" 13.9 1 "a.o declares 13.10" \
  "$(object a.o LC_BUILD_VERSION minos 13.10)"

# The ceiling is the one passed, not the pod's. The arm64 simulator slice is
# held to 14.0, which a ceiling of 13.0 refuses.
expect "the simulator floor under its own ceiling" 14.0 0 "" \
  "$(object a.o LC_BUILD_VERSION minos 14.0)"
expect "the simulator floor under the pod's ceiling" 13.0 1 "a.o declares 14.0" \
  "$(object a.o LC_BUILD_VERSION minos 14.0)"

# `sdk 26.5` follows every stamp, LC_SOURCE_VERSION has a `version` of its
# own, and LC_BUILD_VERSION lists each tool with one. None is a minimum OS.
expect "a version that is not a minimum OS" 13.0 0 "" \
  "$(object a.o LC_BUILD_VERSION minos 13.0
     printf '   ntools 1\n     tool 3\n  version 1230.1\n'
     object b.o LC_SOURCE_VERSION version 99.0
     object c.o LC_BUILD_VERSION minos 12.0)"

# otool prints `n/a` where it has no value. Read as a number that is zero.
expect "a stamp that is not a version" 13.0 1 \
  "a.o declares a minimum OS that is not a version: n/a" \
  "$(object a.o LC_BUILD_VERSION minos n/a)"

# A fat archive names the architecture after the member.
expect "a member of a fat archive" 13.0 1 "ring.o declares 26.5" \
  "$(printf 'lib.a(ring.o) (architecture x86_64):\n      cmd LC_BUILD_VERSION\n    minos 26.5\n')"

# An unreadable archive prints nothing otool-shaped. Passing it would make
# the gate green on the one input it can say nothing about.
expect "no stamp at all" 13.0 2 "" "this is not otool output"

echo "the podspec"

PODSPEC="$WORK/Fixture.podspec"
printf '  s.name = "MeshSdk"\n  s.platforms    = { :ios => "15.1" }\n' >"$PODSPEC"
equal "the version is read" "$(ios_deployment_target "$PODSPEC")" "15.1"

printf '  s.name = "MeshSdk"\n' >"$PODSPEC"
STATUS=0
OUTPUT="$(ios_deployment_target "$PODSPEC" 2>/dev/null)" || STATUS=$?
equal "a podspec that does not say is refused" "$STATUS" "1"
equal "and prints no version" "$OUTPUT" ""

equal "the real podspec says" \
  "$(ios_deployment_target "$REPO_ROOT/bindings/react-native/MeshSdk.podspec" | grep -cE '^[0-9]+\.[0-9]+$')" "1"

# refused <name> <podspec text>
#
# The reader must fail and print no version. Each of these was once read two
# ways: the release build took it, and the Swift package refused it.
refused() {
  local status=0 output
  printf '%b' "$2" >"$PODSPEC"
  output="$(ios_deployment_target "$PODSPEC" 2>/dev/null)" || status=$?
  equal "$1 is refused" "$status" "1"
  equal "$1 prints no version" "$output" ""
}

refused "a bare major version" '  s.platforms = { :ios => "15" }\n'
refused "a version with a trailing dot" '  s.platforms = { :ios => "13." }\n'
refused "two declarations" \
  '  s.platforms = { :ios => "13.0" }\n  s.platforms = { :ios => "15.1" }\n'

printf '  s.platforms = { :ios => "13.4.1" }\n' >"$PODSPEC"
equal "a patch version is read" "$(ios_deployment_target "$PODSPEC")" "13.4.1"

# The script the Swift package and its CI job ask is this function, not a
# second parser. Pointed at a fixture podspec, it answers what the function
# answers, and refuses what the function refuses.
mkdir -p "$WORK/root/bindings/react-native"
printf '  s.platforms = { :ios => "14.2" }\n' >"$WORK/root/bindings/react-native/MeshSdk.podspec"
equal "the package's script reads what the release build reads" \
  "$(PACKAGE_SOURCE_ROOT="$WORK/root" bash "$REPO_ROOT/scripts/ios-deployment-target.sh")" "14.2"
printf '  s.platforms = { :ios => "15" }\n' >"$WORK/root/bindings/react-native/MeshSdk.podspec"
STATUS=0
PACKAGE_SOURCE_ROOT="$WORK/root" bash "$REPO_ROOT/scripts/ios-deployment-target.sh" \
  >/dev/null 2>&1 || STATUS=$?
equal "and refuses what the release build refuses" "$STATUS" "1"

echo "the simulator ceiling"

equal "below the floor, the floor" "$(arm64_simulator_ceiling 13.0)" "14.0"
equal "at the floor" "$(arm64_simulator_ceiling 14.0)" "14.0"
# The pod raised past the floor: the slice is built for the pod's target, and
# a ceiling left at 14.0 would refuse every object in it.
equal "above the floor, the target" "$(arm64_simulator_ceiling 15.1)" "15.1"
equal "a minor compares as a number" "$(newer_version 13.10 13.9)" "13.10"

echo "the gate"

# A stand-in for otool: prints a fixture and exits as told.
STUBS="$WORK/bin"
mkdir -p "$STUBS"
cat >"$STUBS/otool" <<'STUB'
#!/usr/bin/env bash
cat "$OTOOL_FIXTURE"
exit "${OTOOL_STATUS:-0}"
STUB
chmod +x "$STUBS/otool"

# gate <name> <fixture text> <otool status> <ceiling> <want status> <want text>
gate() {
  local name="$1" fixture="$2" otool_status="$3" ceiling="$4"
  local want_status="$5" want_text="$6"
  local status=0

  printf '%s\n' "$fixture" >"$WORK/fixture"
  PATH="$STUBS:$PATH" OTOOL_FIXTURE="$WORK/fixture" OTOOL_STATUS="$otool_status" \
    assert_archive_min_os lib.a "$ceiling" >"$WORK/gate.log" 2>&1 || status=$?

  equal "$name: exit status" "$status" "$want_status"
  if grep -qF -- "$want_text" "$WORK/gate.log"; then
    pass "$name: says '$want_text'"
  else
    fail "$name: does not say '$want_text': $(tr '\n' ' ' <"$WORK/gate.log")"
  fi
}

GOOD="$(object a.o LC_BUILD_VERSION minos 13.0)"
BAD="$(object ring.o LC_BUILD_VERSION minos 26.5)"

gate "a clean archive" "$GOOD" 0 13.0 0 "no object needs an iOS newer than 13.0"
gate "an object that is too new" "$BAD" 0 13.0 1 "ring.o declares 26.5"
gate "an archive with no stamp" "not otool output" 0 13.0 1 "found no minimum OS stamp"
# otool printed the objects it could parse, all of them fine, and then gave
# up. The ones it never reached are the ones nothing looked at.
gate "an archive otool read part of" "$GOOD" 1 13.0 1 "otool could not read all of"

echo "the packaging"

# The gate lives inside package_xcframework, so the release cannot package an
# archive it did not check. These drive the function itself, with stand-ins
# for otool (a fixture per archive), lipo and xcodebuild (which log their
# calls), so a gate that is dropped, skipped or given the wrong ceiling fails
# here whatever the build script around it looks like.
cat >"$STUBS/otool" <<'STUB'
#!/usr/bin/env bash
cat "$2.otool"
STUB
cat >"$STUBS/lipo" <<'STUB'
#!/usr/bin/env bash
echo "lipo $*" >>"$PACKAGING_LOG"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-output" ]; then : >"$2"; fi
  shift
done
STUB
cat >"$STUBS/xcodebuild" <<'STUB'
#!/usr/bin/env bash
echo "xcodebuild $*" >>"$PACKAGING_LOG"
STUB
chmod +x "$STUBS/otool" "$STUBS/lipo" "$STUBS/xcodebuild"

# archive <name> <minos>: an archive on disk and the otool dump the stand-in
# prints for it.
archive() {
  : >"$WORK/$1"
  object "$1.o" LC_BUILD_VERSION minos "$2" >"$WORK/$1.otool"
}

# package <name> <device minos> <arm64 sim minos> <x86_64 sim minos> <target>
# <want status>. The function runs in a subshell because it sets and clears
# an EXIT trap of its own, which would otherwise replace this script's.
package() {
  local name="$1" status=0
  archive device.a "$2"
  archive sim_arm64.a "$3"
  archive sim_x86_64.a "$4"
  export PACKAGING_LOG="$WORK/packaging.log"
  : >"$PACKAGING_LOG"
  rm -rf "$WORK/out"
  mkdir -p "$WORK/out"
  (
    PATH="$STUBS:$PATH"
    package_xcframework "$WORK/out" "$WORK/device.a" "$WORK/sim_arm64.a" \
      "$WORK/sim_x86_64.a" "$5"
  ) >"$WORK/package.out" 2>&1 || status=$?
  equal "$name: exit status" "$status" "$6"
  if [ "$6" != 0 ]; then
    equal "$name: nothing is packaged" "$(wc -l <"$PACKAGING_LOG" | tr -d ' ')" "0"
  fi
}

package "every archive at the target" 13.0 14.0 13.0 13.0 0
equal "one simulator archive is made" "$(grep -c '^lipo -create' "$PACKAGING_LOG")" "1"
equal "one XCFramework is made" "$(grep -c '^xcodebuild -create-xcframework' "$PACKAGING_LOG")" "1"
equal "of two slices" "$(grep -o -- '-library' "$PACKAGING_LOG" | wc -l | tr -d ' ')" "2"

package "a device archive built for a newer iOS" 26.5 14.0 13.0 13.0 1
# The arm64 simulator slice is held to 14.0, never higher.
package "an arm64 simulator archive above its floor" 13.0 14.5 13.0 13.0 1
# The floor is the arm64 simulator's alone: the Intel slice keeps the target.
package "an Intel simulator archive at the arm64 floor" 13.0 14.0 14.0 13.0 1
package "no deployment target" 13.0 14.0 13.0 "" 1
if grep -qF "needs the deployment target" "$WORK/package.out"; then
  pass "no deployment target: says it needs one"
else
  fail "no deployment target: does not say it needs one: $(tr '\n' ' ' <"$WORK/package.out")"
fi

echo "the wiring"

# The build script reads the target, exports it before building, and hands it
# to the packaging, which checks against it.
BUILD="$RN_SCRIPTS/build-uniffi-ios.sh"
line_of() { grep -nF -- "$1" "$BUILD" | head -1 | cut -d: -f1; }

EXPORT_LINE="$(line_of 'export IPHONEOS_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET"')"
READ_LINE="$(line_of 'IOS_DEPLOYMENT_TARGET="$(ios_deployment_target ')"
BUILD_LINE="$(line_of 'cargo build --release --target "$arch"')"

if [ -n "$READ_LINE" ] && [ -n "$EXPORT_LINE" ] && [ "$READ_LINE" -lt "$EXPORT_LINE" ]; then
  pass "the target is read from the podspec, then exported"
else
  fail "the target is not read from the podspec before it is exported"
fi

if [ -n "$EXPORT_LINE" ] && [ -n "$BUILD_LINE" ] && [ "$EXPORT_LINE" -lt "$BUILD_LINE" ]; then
  pass "it is exported before anything is built"
else
  fail "it is not exported before the build"
fi

if awk '/^package_xcframework /{p=1} p{print} p&&!/\\$/{exit}' "$BUILD" |
  grep -qF '"$IOS_DEPLOYMENT_TARGET"'; then
  pass "the packaging is handed the target it checks against"
else
  fail "the packaging is not handed the target"
fi

if [ "$ASSERTIONS" != "$EXPECTED_ASSERTIONS" ]; then
  echo "  FAIL: ran $ASSERTIONS assertions, expected $EXPECTED_ASSERTIONS" >&2
  FAILURES=$((FAILURES + 1))
fi

if [ "$FAILURES" -gt 0 ]; then
  echo "$FAILURES failure(s)" >&2
  exit 1
fi

echo "all $ASSERTIONS assertions passed"
