#!/usr/bin/env python3
"""Check that every key a React Native adapter sends is one its bridge reads.

The adapters (ProtocolEngineHostBridgeReactNative.swift and .kt) are generated
and key each argument by its parameter name; the shared bridges read keys by
hand. Nothing else ties the two, so a mismatch compiles, passes every test and
drops the argument: in 0.31.0 an iPhone's `createInvite` sent `sign` to a bridge
reading `signed`, and `checkInternetPresence` sent `options` on both platforms to
bridges reading `force`.

A key counts as read when its quoted literal is in the handler, or in a same-file
helper the handler hands the argument map to. A helper that takes the map under
another name, or a read through a key held in a variable, is not seen. Run it with
no arguments; exit status is non-zero on any key not read.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
IOS = REPO / "bindings/react-native/ios"
ANDROID = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol"

# (adapter, bridge, one adapter call: method and argument list,
#  the start of one bridge member, one key in the argument list)
PLATFORMS = [
    (
        IOS / "ProtocolEngineHostBridgeReactNative.swift",
        IOS / "ProtocolEngineHostBridge.swift",
        r"ProtocolEngineHostBridge\.(\w+)\(host: host, env: env, arguments: \[(.*)\]\)\n",
        r"^    (?:private |fileprivate )?static func (\w+)\(",
        r'"(\w+)":',
    ),
    (
        ANDROID / "ProtocolEngineHostBridgeReactNative.kt",
        ANDROID / "ProtocolEngineHostBridge.kt",
        r"ProtocolEngineHostBridge\.(\w+)\(host, context, mapOf\((.*)\)\)\n",
        r"^    (?:private |internal )?fun (\w+)\(",
        r'"(\w+)" to ',
    ),
]

# A call handing the argument map to a function, as its first argument.
HANDED_THE_MAP = re.compile(r"\b(\w+)\(\s*(?:m|map)\s*[,)]")
LINE_COMMENT = re.compile(r"^\s*//.*$", re.M)


def members(bridge_text: str, member: str) -> dict[str, str]:
    """Each bridge member's text by name, comment lines removed, overloads joined."""
    text = LINE_COMMENT.sub("", bridge_text)
    starts = list(re.finditer(member, text, re.M))
    out: dict[str, str] = {}
    for start, following in zip(starts, starts[1:] + [None]):
        stop = following.start() if following else len(text)
        out[start.group(1)] = out.get(start.group(1), "") + text[start.start() : stop]
    return out


def read_text_of(name: str, bridge: dict[str, str]) -> str:
    """A handler plus every same-file helper it hands the argument map to.

    Following only those keeps a literal in an unrelated helper, such as one
    building a result map, from passing for a read.
    """
    seen: set[str] = set()
    pending = [name]
    text = ""
    while pending:
        current = pending.pop()
        if current in seen or current not in bridge:
            continue
        seen.add(current)
        text += bridge[current]
        pending += HANDED_THE_MAP.findall(bridge[current])
    return text


def problems() -> list[str]:
    out: list[str] = []
    for adapter, bridge, call, member, key in PLATFORMS:
        adapter_text, bridge_members = adapter.read_text(), members(bridge.read_text(), member)
        calls = re.findall(call, adapter_text)
        if not calls:
            out.append(f"{adapter.name}: no keyed calls found; the pattern is stale")
        for name, arguments in calls:
            if name not in bridge_members:
                out.append(f"{bridge.name}: no handler for {name}")
                continue
            body = read_text_of(name, bridge_members)
            for k in re.findall(key, arguments):
                if f'"{k}"' not in body:
                    out.append(f"{adapter.name}: {name} sends '{k}', which {bridge.name} never reads")
    return out


def self_check() -> list[str]:
    """Prove on a fixture that the search still tells a read from a non-read."""
    member = PLATFORMS[0][3]
    bridge = members(
        """
    static func direct(host: H, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments)
        return m["direct"]
    }

    static func viaHelper(host: H, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments)
        return try helper(m)
    }

    private static func helper(_ m: [String: Any]) throws -> Any? {
        m["helped"]
    }

    static func commentOnly(host: H, arguments: Any?) throws -> Any? {
        // "commented" is what this used to read.
        return nil
    }

    static func unrelated(host: H, arguments: Any?) throws -> Any? {
        let m = try A.map(arguments)
        return result(m.count)
    }

    private static func result(_ count: Int) -> Any? {
        ["unrelated": count]
    }
""",
        member,
    )
    cases = [
        ("direct", '"direct"', True),
        ("viaHelper", '"helped"', True),
        ("direct", '"helped"', False),
        ("commentOnly", '"commented"', False),
        ("unrelated", '"unrelated"', False),
    ]
    return [
        f"self check: {name} reading {key} should be {expected}"
        for name, key, expected in cases
        if (key in read_text_of(name, bridge)) != expected
    ]


def main() -> None:
    found = self_check() + problems()
    for problem in found:
        print(problem, file=sys.stderr)
    if found:
        raise SystemExit(1)
    print("react native adapter argument keys: OK")


if __name__ == "__main__":
    main()
