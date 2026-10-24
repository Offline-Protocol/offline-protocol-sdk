#!/usr/bin/env python3
"""Sync nativeMethodChannelMethods in offline_protocol.dart from method_channel_methods.txt."""

from __future__ import annotations

import pathlib

REPO = pathlib.Path(__file__).resolve().parents[1]
METHODS_FILE = REPO / "bindings/flutter/offline_protocol/method_channel_methods.txt"
DART = REPO / "bindings/flutter/offline_protocol/lib/src/offline_protocol.dart"

MARKER_START = "static const List<String> nativeMethodChannelMethods = ["
MARKER_END = "];"


def main() -> None:
    methods = [ln.strip() for ln in METHODS_FILE.read_text().splitlines() if ln.strip()]
    dart = DART.read_text()
    start = dart.index(MARKER_START)
    end = dart.index(MARKER_END, start)
    body = "\n".join(f"    '{m}'," for m in methods)
    new_block = f"{MARKER_START}\n{body}\n  {MARKER_END}"
    DART.write_text(dart[:start] + new_block + dart[end + len(MARKER_END) :])
    print(f"Updated {DART} with {len(methods)} methods")


if __name__ == "__main__":
    main()
