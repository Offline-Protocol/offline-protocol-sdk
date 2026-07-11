#!/usr/bin/env python3
"""Verify ELF shared libraries are 16 KB page-size aligned.

Google Play requires 16 KB page-size support (PT_LOAD p_align >= 0x4000) for
64-bit native libraries in apps targeting Android 15+. We enforce it on all
ABIs for uniformity. The alignment comes from the per-target rustflags in
.cargo/config.toml; this check exists so a toolchain or config change can
never silently regress it (NDK <= r27 defaults to 4 KB).

Python (stdlib-only) rather than readelf/llvm-objdump so the same check runs
unchanged on macOS dev machines and Linux CI without toolchain lookups.

Usage: check-elf-alignment.py <lib.so> [lib.so ...]
Exits non-zero if any PT_LOAD segment of any input is under-aligned.
"""

import struct
import sys

REQUIRED_ALIGN = 0x4000  # 16 KB

PT_LOAD = 1


def load_aligns(path):
    """Return the p_align of every PT_LOAD program header in an ELF file."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] != b"\x7fELF":
        raise ValueError(f"{path}: not an ELF file")
    is64 = data[4] == 2
    endian = "<" if data[5] == 1 else ">"
    if is64:
        (e_phoff,) = struct.unpack_from(endian + "Q", data, 0x20)
        e_phentsize, e_phnum = struct.unpack_from(endian + "HH", data, 0x36)
    else:
        (e_phoff,) = struct.unpack_from(endian + "I", data, 0x1C)
        e_phentsize, e_phnum = struct.unpack_from(endian + "HH", data, 0x2A)
    aligns = []
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        (p_type,) = struct.unpack_from(endian + "I", data, off)
        if p_type == PT_LOAD:
            if is64:
                (p_align,) = struct.unpack_from(endian + "Q", data, off + 0x30)
            else:
                (p_align,) = struct.unpack_from(endian + "I", data, off + 0x1C)
            aligns.append(p_align)
    if not aligns:
        raise ValueError(f"{path}: no PT_LOAD segments found")
    return aligns


def main(argv):
    if not argv:
        print("usage: check-elf-alignment.py <lib.so> [lib.so ...]", file=sys.stderr)
        return 2
    failed = False
    for path in argv:
        try:
            aligns = load_aligns(path)
        except (OSError, ValueError, struct.error) as e:
            print(f"ERROR {e}")
            failed = True
            continue
        shown = ", ".join(hex(a) for a in aligns)
        if all(a >= REQUIRED_ALIGN for a in aligns):
            print(f"OK    {path}: PT_LOAD align [{shown}]")
        else:
            print(
                f"FAIL  {path}: PT_LOAD align [{shown}] — "
                f"all segments must be >= {hex(REQUIRED_ALIGN)} (16 KB pages)"
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
