#!/usr/bin/env python3
"""Check the hand-written iOS ProtocolEngineHostBridge.swift.

The Kotlin bridge is generated and `gen_protocol_engine_host_bridge.py --check`
holds it. The Swift bridge is written by hand, so nothing regenerates it and a
missing or stubbed handler compiles and ships. This holds the two invariants a
generator would have:

* every name in `METHODS` (tools/gen_protocol_engine_host_bridge.py, the list
  the React Native adapters delegate) and in the Flutter method-channel
  manifest (bindings/flutter/offline_protocol/method_channel_methods.txt, which
  the generated FlutterEngineChannel.swift dispatches) has exactly one
  `static func` with the one handler signature, and no other handler exists;
* no handler is a stub: none throws "not implemented", calls `fatalError`, or
  carries a TODO or FIXME.

Exit status is non-zero on any violation. Run it with no arguments.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
GENERATOR = REPO / "tools/gen_protocol_engine_host_bridge.py"
MANIFEST = REPO / "bindings/flutter/offline_protocol/method_channel_methods.txt"
BRIDGE = REPO / "bindings/react-native/ios/ProtocolEngineHostBridge.swift"

SIGNATURE = re.compile(
    r"^    static func (\w+)\(host: ProtocolEngineHost, "
    r"env: EngineBridgeEnvironment, arguments: Any\?\) throws -> Any\? \{$",
    re.MULTILINE,
)
STUB_MARKERS = ("not implemented", "notImplemented", "fatalError(", "TODO", "FIXME")


def method_names() -> list[str]:
    source = GENERATOR.read_text()
    body = source.split("METHODS = [", 1)[1].split("]", 1)[0]
    return re.findall(r'^\s*"(\w+)",?\s*$', body, re.MULTILINE)


def manifest_names() -> list[str]:
    return [ln.strip() for ln in MANIFEST.read_text().splitlines() if ln.strip()]


def main() -> int:
    methods = method_names()
    if not methods:
        print(f"no METHODS found in {GENERATOR.relative_to(REPO)}", file=sys.stderr)
        return 1
    manifest = manifest_names()
    if not manifest:
        print(f"no methods found in {MANIFEST.relative_to(REPO)}", file=sys.stderr)
        return 1
    wanted = set(methods) | set(manifest)
    source = BRIDGE.read_text()
    matches = list(SIGNATURE.finditer(source))
    problems: list[str] = []

    counts: dict[str, int] = {}
    for m in matches:
        counts[m.group(1)] = counts.get(m.group(1), 0) + 1
    for name in sorted(wanted):
        n = counts.get(name, 0)
        if n != 1:
            problems.append(f"`{name}` has {n} handlers with the bridge signature, expected 1")
    for name in sorted(set(counts) - wanted):
        problems.append(f"`{name}` is a handler no manifest names, so nothing dispatches it")

    for i, m in enumerate(matches):
        end = matches[i + 1].start() if i + 1 < len(matches) else len(source)
        body = source[m.end():end]
        for marker in STUB_MARKERS:
            if marker in body:
                problems.append(f"`{m.group(1)}` looks like a stub: contains `{marker}`")

    if problems:
        for p in problems:
            print(f"{BRIDGE.relative_to(REPO)}: {p}", file=sys.stderr)
        return 1
    print(f"{BRIDGE.name}: OK ({len(wanted)} methods)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
