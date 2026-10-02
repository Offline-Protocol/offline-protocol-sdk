#!/usr/bin/env bash

# Drives bindings/python/scripts/build-wheel.sh on a copy of the Python
# package, with a shared library compiled here whose glibc floor is known.
#
# A platform tag is a claim about a wheel nothing downstream re-checks: a tag
# older than the library's real floor installs on a system where the library
# then fails to load, and pip on the release's test runner, which is newer,
# accepts it. So each claim is pinned here against a library whose answer is
# known: the glibc floor from a library that asks for getrandom (GLIBC_2.25)
# and nothing newer, the GLIBC_ABI_* markers that name no symbol, the
# architecture from the header, and the macOS minimum and the Windows machine
# from hand-built headers, so neither needs a Mac or a Windows runner. Needs
# gcc, readelf and Python with `build` and `wheel`: a Linux runner.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FAILURES=0
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass() { echo "  ok - $*"; }
fail() {
  echo "  FAIL - $*" >&2
  FAILURES=$((FAILURES + 1))
}

# The script edits pyproject.toml and the package directory in place.
copy_package() {
  local root="$1"
  mkdir -p "$root/scripts" "$root/bindings"
  cp "$REPO_ROOT/scripts/pep440-version.sh" "$root/scripts/"
  (cd "$REPO_ROOT/bindings" && tar -cf - --exclude build --exclude dist --exclude '*.egg-info' \
    --exclude __pycache__ --exclude '*.so' --exclude '*.dylib' --exclude '*.dll' python) |
    (cd "$root/bindings" && tar -xf -)
}

# Runs build-wheel.sh on a fresh copy and expects a refusal that names $2.
expect_refusal() {
  local label="$1" reason="$2" platform="$3" library="$4" dir
  dir="$WORK/refuse-$(echo "$label" | tr -c 'a-z0-9' '-')"
  copy_package "$dir"
  if (cd "$dir" && bash bindings/python/scripts/build-wheel.sh "$platform" "$library" 1.2.3) >"$dir.log" 2>&1; then
    fail "$label: wheeled"
  elif grep -q -- "$reason" "$dir.log"; then
    pass "$label: refused"
  else
    fail "$label: refused for another reason"
    cat "$dir.log" >&2
  fi
}

# Runs build-wheel.sh on a fresh copy and expects the wheel file name $4.
expect_wheel() {
  local label="$1" platform="$2" library="$3" want="$4" dir
  dir="$WORK/build-$(echo "$label" | tr -c 'a-z0-9' '-')"
  copy_package "$dir"
  if (cd "$dir" && bash bindings/python/scripts/build-wheel.sh "$platform" "$library" 1.2.3) >"$dir.log" 2>&1; then
    if [ "$(basename "$(tail -1 "$dir.log")")" = "$want" ]; then
      pass "$label: $want"
    else
      fail "$label: the wheel is $(basename "$(tail -1 "$dir.log")"), expected $want"
    fi
  else
    fail "$label: refused"
    cat "$dir.log" >&2
  fi
}

ARCH="$(uname -m)"
cat >"$WORK/floor.c" <<'C'
#include <sys/random.h>
long offline_fixture(void *buffer) { return getrandom(buffer, 8, 0); }
C
gcc -shared -fPIC -o "$WORK/libfixture.so" "$WORK/floor.c"
cat >"$WORK/zlib.c" <<'C'
#include <zlib.h>
unsigned long offline_fixture(void) { return zlibCompileFlags(); }
C
HAVE_ZLIB=0
gcc -shared -fPIC -o "$WORK/libzfixture.so" "$WORK/zlib.c" -lz 2>/dev/null && HAVE_ZLIB=1

echo "== the tag is read from the library =="
copy_package "$WORK/good"
if (cd "$WORK/good" && bash bindings/python/scripts/build-wheel.sh "linux-$ARCH" "$WORK/libfixture.so" 1.2.3-rc.4) >"$WORK/good.log" 2>&1; then
  WHEEL="$(tail -1 "$WORK/good.log")"
  NEWEST="$(readelf --dyn-syms -W "$WORK/libfixture.so" | grep -o 'GLIBC_2\.[0-9]*' | sort -t. -k2 -n -u | tail -1)"
  WANT="offline_protocol_sdk-1.2.3rc4-py3-none-manylinux_2_${NEWEST#GLIBC_2.}_$ARCH.whl"
  if [ "$(basename "$WHEEL")" = "$WANT" ]; then
    pass "tagged $WANT"
  else
    fail "the wheel is $(basename "$WHEEL"), expected $WANT"
  fi
  # getrandom arrived in 2.25, so the floor cannot be lower.
  if [ "${NEWEST#GLIBC_2.}" -ge 25 ]; then
    pass "the floor covers getrandom (GLIBC_2.25)"
  else
    fail "the floor $NEWEST is below the getrandom the library calls"
  fi
  if python3 -m zipfile -l "$WHEEL" | grep -q 'offline_protocol_sdk/libuniffi.so'; then
    pass "the library is in the wheel under the loader's name"
  else
    fail "the wheel has no offline_protocol_sdk/libuniffi.so"
  fi
else
  fail "building a wheel from a manylinux library failed"
  cat "$WORK/good.log" >&2
fi

echo "== refusals =="
if [ "$HAVE_ZLIB" = 1 ]; then
  copy_package "$WORK/zlib"
  if (cd "$WORK/zlib" && bash bindings/python/scripts/build-wheel.sh "linux-$ARCH" "$WORK/libzfixture.so" 1.2.3) >"$WORK/zlib.log" 2>&1; then
    fail "a library that links libz was wheeled"
  elif grep -q 'outside the manylinux set' "$WORK/zlib.log"; then
    pass "a library that links libz is refused"
  else
    fail "a library that links libz was refused for another reason"
    cat "$WORK/zlib.log" >&2
  fi
else
  echo "  skip - no zlib headers to build the refusal fixture"
fi
copy_package "$WORK/badversion"
if (cd "$WORK/badversion" && bash bindings/python/scripts/build-wheel.sh "linux-$ARCH" "$WORK/libfixture.so" 1.2.3+local) >/dev/null 2>&1; then
  fail "a version with no Python spelling was wheeled"
else
  pass "a version with no Python spelling is refused"
fi
copy_package "$WORK/badplatform"
if (cd "$WORK/badplatform" && bash bindings/python/scripts/build-wheel.sh linux-riscv64 "$WORK/libfixture.so" 1.2.3) >/dev/null 2>&1; then
  fail "an unknown platform was wheeled"
else
  pass "an unknown platform is refused"
fi

echo "== the floor counts the glibc ABI markers =="
# A library with packed relative relocations needs GLIBC_ABI_DT_RELR, which
# names no symbol, so the symbol floor alone (2.25 here) would claim a glibc
# that cannot load it. GNU ld adds the need against glibc 2.36 or newer, on
# x86_64 from binutils 2.38 (the release runner) and on aarch64 only from 2.43,
# so Ubuntu 24.04 on arm skips this.
cat >"$WORK/relr.c" <<'C'
#include <sys/random.h>
static int a, b, c;
int *pointers[] = { &a, &b, &c };
long offline_fixture(void *buffer) { return getrandom(buffer, 8, 0); }
C
if gcc -shared -fPIC -Wl,-z,pack-relative-relocs -o "$WORK/librelr.so" "$WORK/relr.c" 2>/dev/null &&
  readelf -V -W "$WORK/librelr.so" | grep -q 'Name: GLIBC_ABI_DT_RELR'; then
  expect_wheel "a library that needs GLIBC_ABI_DT_RELR" "linux-$ARCH" "$WORK/librelr.so" \
    "offline_protocol_sdk-1.2.3-py3-none-manylinux_2_36_$ARCH.whl"
else
  echo "  skip - this linker or glibc does not emit GLIBC_ABI_DT_RELR"
fi

# The markers no toolchain here emits on demand, through a readelf whose
# version listing is replaced; everything else it is asked is the real one.
REAL_READELF="$(command -v readelf)"
mkdir -p "$WORK/fakebin"
cat >"$WORK/fakebin/readelf" <<SH
#!/bin/sh
if [ "\$1" = "-V" ]; then cat "\$FAKE_VERSION_NEEDS"; exit 0; fi
exec "$REAL_READELF" "\$@"
SH
chmod +x "$WORK/fakebin/readelf"
version_needs() {
  cat >"$WORK/needs-$1.txt" <<TXT

Version needs section '.gnu.version_r' contains 2 entries:
 Addr: 0x0000000000000320  Offset: 0x00000320  Link: 4 (.dynstr)
  000000: Version: 1  File: libgcc_s.so.1  Cnt: 1
  0x0010:   Name: GCC_3.0  Flags: none  Version: 4
  0x0020: Version: 1  File: libc.so.6  Cnt: 3
  0x0030:   Name: GLIBC_2.2.5  Flags: none  Version: 3
  0x0040:   Name: GLIBC_2.34  Flags: none  Version: 2
  0x0050:   Name: $2  Flags: none  Version: 5

Version definition section '.gnu.version_d' contains 1 entry:
 Addr: 0x0000000000000300  Offset: 0x00000300  Link: 4 (.dynstr)
  000000: Rev: 1  Flags: BASE  Index: 1  Cnt: 1  Name: GLIBC_2.99
TXT
}
with_needs() {
  local name="$1"
  shift
  PATH="$WORK/fakebin:$PATH" FAKE_VERSION_NEEDS="$WORK/needs-$name.txt" "$@"
}
version_needs tls GLIBC_ABI_GNU2_TLS
version_needs plt GLIBC_ABI_DT_X86_64_PLT
version_needs future GLIBC_ABI_SOMETHING_NEW
version_needs private GLIBC_PRIVATE
version_needs plain GLIBC_2.17
with_needs tls expect_wheel "GLIBC_ABI_GNU2_TLS" "linux-$ARCH" "$WORK/libfixture.so" \
  "offline_protocol_sdk-1.2.3-py3-none-manylinux_2_42_$ARCH.whl"
with_needs plt expect_wheel "GLIBC_ABI_DT_X86_64_PLT" "linux-$ARCH" "$WORK/libfixture.so" \
  "offline_protocol_sdk-1.2.3-py3-none-manylinux_2_42_$ARCH.whl"
# The version definition after the needs names GLIBC_2.99: a library's own
# versions are not needs, so the reading stops where the needs section ends.
with_needs plain expect_wheel "the floor reads only the version needs" "linux-$ARCH" "$WORK/libfixture.so" \
  "offline_protocol_sdk-1.2.3-py3-none-manylinux_2_34_$ARCH.whl"
with_needs future expect_refusal "an unknown glibc ABI marker" "does not know the floor" \
  "linux-$ARCH" "$WORK/libfixture.so"
with_needs private expect_refusal "GLIBC_PRIVATE" "no manylinux policy allows" \
  "linux-$ARCH" "$WORK/libfixture.so"

echo "== the architecture is read from the library =="
case "$ARCH" in
  x86_64) OTHER_ARCH=aarch64 ;;
  *) OTHER_ARCH=x86_64 ;;
esac
expect_refusal "a $ARCH library as linux-$OTHER_ARCH" "ELF machine" "linux-$OTHER_ARCH" "$WORK/libfixture.so"
expect_refusal "a Linux library as macos-arm64" "not a 64-bit little-endian Mach-O" macos-arm64 "$WORK/libfixture.so"
expect_refusal "a Linux library as windows-x86_64" "not a PE library" windows-x86_64 "$WORK/libfixture.so"

echo "== the macOS minimum and the Windows machine =="
# Hand-built headers, enough of each format for the check to read.
python3 - "$WORK" <<'PYTHON'
import struct, sys
work = sys.argv[1]

def macho(name, cputype=0x0100000C, commands=None, magic=0xFEEDFACF):
    commands = commands if commands is not None else [build_version(11, 0)]
    body = b"".join(commands)
    header = struct.pack("<IiiIIIII", magic, cputype, 0, 6, len(commands), len(body), 0, 0)
    with open(f"{work}/{name}", "wb") as f:
        f.write(header + body + bytes(64))

def build_version(major, minor, platform=1):
    return struct.pack("<IIIIII", 0x32, 24, platform, (major << 16) | (minor << 8), 15 << 16, 0)

def version_min(major, minor):
    return struct.pack("<IIII", 0x24, 16, (major << 16) | (minor << 8), 15 << 16)

macho("macho-11.dylib")
macho("macho-14.dylib", commands=[build_version(14, 0)])
macho("macho-legacy.dylib", commands=[version_min(10, 15)])
macho("macho-15.dylib", commands=[build_version(15, 0)])
macho("macho-ios.dylib", commands=[build_version(13, 0, platform=2)])
macho("macho-x86_64.dylib", cputype=0x01000007)
macho("macho-nominos.dylib", commands=[struct.pack("<II16s", 0x1B, 24, bytes(16))])
with open(f"{work}/macho-fat.dylib", "wb") as f:
    f.write(struct.pack(">II", 0xCAFEBABE, 2) + bytes(64))

def pe(name, machine, signature=b"PE\0\0"):
    data = bytearray(0x80)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x40)
    data[0x40:0x44] = signature
    struct.pack_into("<H", data, 0x44, machine)
    with open(f"{work}/{name}", "wb") as f:
        f.write(bytes(data))

pe("pe-x86_64.dll", 0x8664)
pe("pe-i386.dll", 0x014C)
pe("pe-arm64.dll", 0xAA64)
# A DOS stub with no PE header behind it, as a 16-bit executable has.
pe("pe-nosignature.dll", 0x8664, signature=b"NE\0\0")
PYTHON
MAC_WHEEL="offline_protocol_sdk-1.2.3-py3-none-macosx_14_0_arm64.whl"
expect_wheel "a library that needs macOS 11.0" macos-arm64 "$WORK/macho-11.dylib" "$MAC_WHEEL"
expect_wheel "a library that needs macOS 14.0" macos-arm64 "$WORK/macho-14.dylib" "$MAC_WHEEL"
expect_wheel "a library with LC_VERSION_MIN_MACOSX" macos-arm64 "$WORK/macho-legacy.dylib" "$MAC_WHEEL"
expect_refusal "a library that needs macOS 15.0" "newer than the 14.0 the wheel claims" macos-arm64 "$WORK/macho-15.dylib"
expect_refusal "an iOS library" "not macOS" macos-arm64 "$WORK/macho-ios.dylib"
expect_refusal "an x86_64 Mach-O" "expected arm64" macos-arm64 "$WORK/macho-x86_64.dylib"
expect_refusal "a Mach-O with no minimum" "minimum macOS is unknown" macos-arm64 "$WORK/macho-nominos.dylib"
expect_refusal "a universal binary" "universal binary" macos-arm64 "$WORK/macho-fat.dylib"
expect_wheel "an x86_64 DLL" windows-x86_64 "$WORK/pe-x86_64.dll" \
  "offline_protocol_sdk-1.2.3-py3-none-win_amd64.whl"
expect_refusal "an i386 DLL" "expected x86_64" windows-x86_64 "$WORK/pe-i386.dll"
expect_refusal "an arm64 DLL" "expected x86_64" windows-x86_64 "$WORK/pe-arm64.dll"
expect_refusal "a DOS stub with no PE header" "not a PE library" windows-x86_64 "$WORK/pe-nosignature.dll"
expect_refusal "a DLL as linux-$ARCH" "not an ELF library" "linux-$ARCH" "$WORK/pe-x86_64.dll"

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "All wheel checks passed."
else
  echo "$FAILURES wheel check(s) failed." >&2
  exit 1
fi
