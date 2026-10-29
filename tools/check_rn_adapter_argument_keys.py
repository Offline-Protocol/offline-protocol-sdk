#!/usr/bin/env python3
"""Check that every key a React Native adapter sends is one its bridge reads.

The adapters (ProtocolEngineHostBridgeReactNative.swift and .kt) are generated
and key each argument by its parameter name; the shared bridges read keys by
hand. Nothing else ties the two, so a mismatch compiles, passes every test and
drops the argument: in 0.31.0 an iPhone's `createInvite` sent `sign` to a bridge
reading `signed`, and `checkInternetPresence` sent `options` on both platforms to
bridges reading `force`. Run it with no arguments; exit status is non-zero on any
key a bridge never names.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
IOS = REPO / "bindings/react-native/ios"
ANDROID = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol"

# (adapter, bridge, one adapter call: method and argument list,
#  one bridge handler: method, one key in the argument list)
PLATFORMS = [
    (
        IOS / "ProtocolEngineHostBridgeReactNative.swift",
        IOS / "ProtocolEngineHostBridge.swift",
        r"ProtocolEngineHostBridge\.(\w+)\(host: host, env: env, arguments: \[(.*)\]\)\n",
        r"\n    static func {name}\(host:.*?\n    }}\n",
        r'"(\w+)":',
    ),
    (
        ANDROID / "ProtocolEngineHostBridgeReactNative.kt",
        ANDROID / "ProtocolEngineHostBridge.kt",
        r"ProtocolEngineHostBridge\.(\w+)\(host, context, mapOf\((.*)\)\)\n",
        r"\n    fun {name}\(host:.*?\n    }}\n",
        r'"(\w+)" to ',
    ),
]


def problems() -> list[str]:
    out: list[str] = []
    for adapter, bridge, call, handler, key in PLATFORMS:
        adapter_text, bridge_text = adapter.read_text(), bridge.read_text()
        calls = re.findall(call, adapter_text)
        if not calls:
            out.append(f"{adapter.name}: no keyed calls found; the pattern is stale")
        for name, arguments in calls:
            body = re.search(handler.format(name=re.escape(name)), bridge_text, re.S)
            if body is None:
                out.append(f"{bridge.name}: no handler for {name}")
                continue
            for k in re.findall(key, arguments):
                if f'"{k}"' not in body.group(0):
                    out.append(f"{adapter.name}: {name} sends '{k}', which {bridge.name} never reads")
    return out


def main() -> None:
    found = problems()
    for problem in found:
        print(problem, file=sys.stderr)
    if found:
        raise SystemExit(1)
    print("react native adapter argument keys: OK")


if __name__ == "__main__":
    main()
