#!/usr/bin/env python3
"""Check a hand-copied jniLibs tree for the name UniFFI's Kotlin loader opens.

The generated Kotlin loads libuniffi_offline_protocol.so. Cargo writes
liboffline_protocol_uniffi.so, and a copy that keeps the cargo name fails at
the first engine call with UnsatisfiedLinkError: Unable to load library
'uniffi_offline_protocol'. Applications on the Maven library never need this:
the AAR carries the right name.

The tree belongs to the application, so other native libraries in it
(libc++_shared.so, a dependency's own) are fine. What fails:

- the cargo output name anywhere in the tree;
- an ABI directory that holds native libraries but not the loaded name (the
  app crashes on that ABI only, which is the device nobody tested on);
- a tree with no ABI directory holding the loaded name;
- a .so directly under the root, which Android never packages.

Usage:
    python3 scripts/check_manual_android_jni_libs.py <jniLibs-root>

Exit 0 when the tree is right, 1 when it is not, 2 on a usage error or a path
that is not a directory (a mistyped path must not read as a pass).
"""

from __future__ import annotations

import sys
from pathlib import Path

LOADED = "libuniffi_offline_protocol.so"
STALE = "liboffline_protocol_uniffi.so"


def check(root: Path) -> list[str]:
    """Return one line per problem in the jniLibs tree at ``root``."""
    problems: list[str] = []
    for path in sorted(root.rglob(STALE)):
        problems.append(f"{path}: holds {STALE}, which nothing loads; rename it to {LOADED}")
    for path in sorted(root.glob("*.so")):
        problems.append(f"{path}: outside an ABI directory, so Android never packages it")
    shipped = False
    for abi in sorted(p for p in root.iterdir() if p.is_dir()):
        if not any(abi.glob("*.so")):
            continue
        if (abi / LOADED).is_file():
            shipped = True
        else:
            problems.append(f"{abi}: holds native libraries but not {LOADED}")
    if not shipped and not problems:
        problems.append(f"{root}: no ABI directory holds {LOADED}")
    return problems


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <jniLibs-root>", file=sys.stderr)
        return 2
    root = Path(argv[1])
    if not root.is_dir():
        print(f"ERROR: {root} is not a directory", file=sys.stderr)
        return 2
    problems = check(root)
    if problems:
        print("ERROR: jniLibs naming:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print(f"{root}: every ABI holds {LOADED}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
