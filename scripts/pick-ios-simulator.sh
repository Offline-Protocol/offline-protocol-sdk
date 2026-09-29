#!/usr/bin/env bash

# Print the identifier of one available iPhone simulator.
#
# `xcodebuild test` wants a destination, and a destination named by device
# ("iPhone 16") exists only on the Xcode that shipped that device: the name a
# developer's machine has is not the name the CI image has, and neither is the
# name next year's image will have. An identifier read from the machine is
# right wherever it is read.
#
# It picks the newest runtime the active Xcode can run, not the newest one
# installed. A CI image carries runtimes for more than one Xcode, and the
# newest of them belongs to the newest Xcode, which is not always the default.

set -euo pipefail

command -v xcrun >/dev/null || {
  echo "ERROR: xcrun is not on PATH: simulators exist only on a Mac" >&2
  exit 1
}

SDK_VERSION="$(xcrun --sdk iphonesimulator --show-sdk-version)"

xcrun simctl list devices available --json | SDK_VERSION="$SDK_VERSION" python3 -c '
import json, os, sys


def parse(text, separator):
    try:
        return tuple(int(part) for part in text.split(separator))
    except ValueError:
        return None


# A runtime key ends in its version, as in
# com.apple.CoreSimulator.SimRuntime.iOS-18-6.
def version(runtime):
    tail = runtime.rsplit(".", 1)[-1]
    if not tail.startswith("iOS-"):
        return None
    return parse(tail[len("iOS-"):], "-")


sdk = parse(os.environ["SDK_VERSION"], ".")
if sdk is None:
    sys.stderr.write("ERROR: cannot read the simulator SDK version\n")
    sys.exit(1)

devices = json.load(sys.stdin)["devices"]
runtimes = sorted(
    (r for r in devices if version(r) is not None and version(r)[:2] <= sdk[:2]),
    key=version,
    reverse=True,
)
for runtime in runtimes:
    for device in devices[runtime]:
        if device.get("isAvailable") and device["name"].startswith("iPhone"):
            print(device["udid"])
            sys.exit(0)

sys.stderr.write(
    "ERROR: no available iPhone simulator on a runtime up to iOS %s\n"
    % os.environ["SDK_VERSION"]
)
sys.exit(1)
'
