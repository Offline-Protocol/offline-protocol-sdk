#!/usr/bin/env python3
"""Check iOS ProtocolEngineHostBridge.swift against the method manifest.

This used to generate a file of one-line forwards into the plugin. The bridge
is now hand-written (it ports Android's ProtocolEngineHostBridge.kt and
EngineMethodHandlers.kt), so this tool only verifies the invariants a
generator would have held:

* every name in method_channel_methods.txt has a `static func` with the one
  handler signature, and no other handler exists;
* no handler is a stub: none returns FlutterMethodNotImplemented, throws
  "not implemented", or contains a TODO.

Exit status is non-zero on any violation. Run it with no arguments.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = REPO / "bindings/flutter/offline_protocol/method_channel_methods.txt"
BRIDGE = REPO / "bindings/react-native/ios/ProtocolEngineHostBridge.swift"

SIGNATURE = re.compile(
    r"^    static func (\w+)\(host: ProtocolEngineHost, "
    r"env: EngineBridgeEnvironment, arguments: Any\?\) throws -> Any\? \{$",
    re.MULTILINE,
)
STUB_MARKERS = ("FlutterMethodNotImplemented", "not implemented", "notImplemented", "TODO", "FIXME")


def main() -> int:
    manifest = [ln.strip() for ln in MANIFEST.read_text().splitlines() if ln.strip()]
    source = BRIDGE.read_text()
    defined = SIGNATURE.findall(source)
    problems: list[str] = []

    for name in sorted(set(manifest) - set(defined)):
        problems.append(f"missing handler: {name}")
    for name in sorted(set(defined) - set(manifest)):
        problems.append(f"handler not in the manifest: {name}")
    for name in sorted({n for n in defined if defined.count(n) > 1}):
        problems.append(f"duplicate handler: {name}")
    for marker in STUB_MARKERS:
        if marker in source:
            problems.append(f"stub marker present: {marker}")

    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    print(f"{BRIDGE.name}: {len(manifest)} handlers, all present")
    return 0


if __name__ == "__main__":
    sys.exit(main())
