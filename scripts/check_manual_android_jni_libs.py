#!/usr/bin/env python3
"""Refuse a jniLibs tree that holds the cargo output name instead of the loaded name.

UniFFI's Kotlin loader opens libuniffi_offline_protocol.so. Cargo writes
liboffline_protocol_uniffi.so. Manual integrators copy the latter by mistake.

Usage:
    python3 scripts/check_manual_android_jni_libs.py <jniLibs-root>

Exit 0 when the directory is absent, empty of .so files, or every .so is the
loaded name. Exit 1 otherwise.
"""

from __future__ import annotations

import sys
from pathlib import Path

LOADED = "libuniffi_offline_protocol.so"
STALE = "liboffline_protocol_uniffi.so"


def check(root: Path) -> list[str]:
    problems: list[str] = []
    if not root.is_dir():
        return problems
    for path in sorted(root.rglob("*.so")):
        name = path.name
        if name == STALE:
            problems.append(
                f"{path}: holds {STALE}, which nothing loads; rename to {LOADED}"
            )
        elif name != LOADED:
            problems.append(
                f"{path}: unexpected native library name (expected {LOADED})"
            )
    return problems


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__.split("\n", 1)[0], file=sys.stderr)
        print(f"usage: {argv[0]} <jniLibs-root>", file=sys.stderr)
        return 2
    root = Path(argv[1])
    problems = check(root)
    if problems:
        print("ERROR: jniLibs naming:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print(f"{root}: ok (no stale cargo .so names)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
