#!/usr/bin/env bash

# Build the platform wheel for one desktop library.
#
# The package is pure Python plus one native library it loads with ctypes, so
# setuptools calls it `py3-none-any`: a wheel that claims to install anywhere.
# Four of them, one per platform, would then share one file name with a
# different library inside, and pip would install a Windows DLL on Linux
# without a word. So the wheel is retagged for the platform its library was
# built for, and pip refuses it everywhere else.
#
# Every claim the tag makes is checked against the library, because nothing
# after this script re-checks one: pip on a runner newer than the claim
# accepts a claim that is too old, and the library then fails to load on the
# user's machine, at import rather than at install.
#
# - The architecture is read from the library's header (ELF, Mach-O, PE), not
#   assumed from the platform argument, so a library from the wrong build
#   cannot be wheeled under a correct-looking name.
# - The Linux glibc floor is read from the library, never written down. A
#   manylinux tag promises the glibc floor it names; the library carries the
#   floor of the image that built it. The floor counts every version the
#   library needs from glibc, including the GLIBC_ABI_* markers, which name no
#   symbol: GLIBC_ABI_DT_RELR (packed relative relocations) needs glibc 2.36
#   whatever the symbols say, and a marker this script does not know is
#   refused rather than ignored. The script also refuses a library that links
#   anything outside the manylinux set, which no system can be assumed to have.
# - The macOS floor is a choice, MACOS_FLOOR below, and the library's own
#   minimum (LC_BUILD_VERSION) must not exceed it.
#
# The wheel's version is the release's, in Python's spelling
# (scripts/pep440-version.sh), so a release candidate is never numbered as the
# release it rehearses.
#
# Usage:
#   bash bindings/python/scripts/build-wheel.sh <platform> <library> <version>
#
#   <platform>  macos-arm64 | linux-x86_64 | linux-aarch64 | windows-x86_64
#   <library>   the built library, under its cargo name
#   <version>   the release version, e.g. 0.28.0 or 0.28.0-rc.1
#
# Writes exactly one wheel to bindings/python/dist/, and prints its path last.
# Edits bindings/python/pyproject.toml (the version) and the package directory
# (the library) in place: it is for a throwaway checkout.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PY_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$PY_ROOT/../.." && pwd)"
PKG_DIR="$PY_ROOT/offline_protocol_sdk"

die() {
  echo "ERROR: $*" >&2
  exit 1
}

[ "$#" -eq 3 ] || die "usage: $0 <platform> <library> <version>"
PLATFORM="$1"
LIBRARY="$2"
VERSION="$3"

[ -f "$LIBRARY" ] || die "no such library: $LIBRARY"
[ -s "$PKG_DIR/offline_protocol.py" ] || die "the generated bindings are missing: $PKG_DIR/offline_protocol.py"

PY_VERSION="$(bash "$REPO_ROOT/scripts/pep440-version.sh" "$VERSION")"

# The libraries the manylinux policies allow a wheel to take from the system.
# A Rust cdylib needs libc, libm, libgcc_s and the loader; the rest are listed
# so a toolchain change that adds one of them is not refused for nothing.
MANYLINUX_LIBS=(libc.so.6 libm.so.6 libdl.so.2 librt.so.1 libpthread.so.0
  libgcc_s.so.1 libstdc++.so.6 libutil.so.1 libresolv.so.2
  ld-linux-x86-64.so.2 ld-linux-aarch64.so.1)

# The oldest macOS the macOS wheel claims. 14 is the oldest arm64 runner the
# suite runs on (macos-14), so it is the oldest macOS the wheel is known to
# work on. The library itself declares Rust's default, 11.0; that is a floor
# nothing has tested, so it is not claimed. A library that declares a NEWER
# minimum than this is refused, since the wheel would then install on a macOS
# where the library cannot load.
MACOS_FLOOR=14.0

# The glibc release a GLIBC_ABI_* marker needs. These are version needs with no
# symbol behind them: the static linker adds one when the library uses a
# feature older loaders mishandle, so they never show in a symbol listing.
# Same numbers as auditwheel's manylinux policies.
glibc_abi_marker_floor() {
  case "$1" in
    GLIBC_ABI_DT_RELR) echo 36 ;;
    GLIBC_ABI_GNU2_TLS | GLIBC_ABI_DT_X86_64_PLT) echo 42 ;;
    *) return 1 ;;
  esac
}

# manylinux_2_<N>_<arch>, N the newest glibc any of the library's version
# needs asks for.
manylinux_tag() {
  local arch="$1" needed newest=0 name minor
  command -v readelf >/dev/null || die "readelf is needed to read the library's glibc floor"

  while IFS= read -r needed; do
    local allowed=0 lib
    for lib in "${MANYLINUX_LIBS[@]}"; do
      [ "$needed" = "$lib" ] && allowed=1
    done
    [ "$allowed" = 1 ] || die "the library links $needed, which is outside the manylinux set"
  done < <(readelf -d -W "$LIBRARY" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')

  # Every name in the version needs section, from any file: libc, libm and the
  # loader all carry glibc versions. Version definitions, which name the
  # library's own versions, are a different section and are not read.
  local version_needs
  version_needs="$(readelf -V -W "$LIBRARY")" || die "readelf could not read the library's version needs"
  while IFS= read -r name; do
    case "$name" in
      GLIBC_2.*)
        minor="${name#GLIBC_2.}"
        minor="${minor%%.*}"
        [[ "$minor" =~ ^[0-9]+$ ]] || die "the library needs $name, which is not a glibc version this script can read"
        ;;
      GLIBC_ABI_*)
        minor="$(glibc_abi_marker_floor "$name")" ||
          die "the library needs $name, a glibc ABI marker this script does not know the floor of"
        ;;
      GLIBC_*)
        die "the library needs $name, which no manylinux policy allows"
        ;;
      *)
        continue
        ;;
    esac
    if [ "$minor" -gt "$newest" ]; then newest="$minor"; fi
  done < <(printf '%s\n' "$version_needs" |
    awk '/^Version needs section/ { needs = 1; next }
         /^Version (definition|symbols) section/ { needs = 0 }
         needs { for (i = 1; i < NF; i++) if ($i == "Name:") print $(i + 1) }')

  [ "$newest" -gt 0 ] || die "no GLIBC_2.N version need in the library: is it a glibc build?"
  echo "manylinux_2_${newest}_$arch"
}

# The library is the one the platform names: its format and architecture
# from its header, and on macOS its minimum OS against the floor. Printed for
# the log.
python - "$LIBRARY" "$PLATFORM" "$MACOS_FLOOR" >&2 <<'PYTHON' || die "the library does not match $PLATFORM"
import struct, sys

path, platform, floor = sys.argv[1], sys.argv[2], sys.argv[3]
with open(path, "rb") as f:
    data = f.read()

def refuse(why):
    print(f"ERROR: {path}: {why}", file=sys.stderr)
    sys.exit(1)

def elf(machine_wanted, name):
    if data[:4] != b"\x7fELF":
        refuse("not an ELF library")
    if data[4] != 2 or data[5] != 1:
        refuse("not a 64-bit little-endian ELF library")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != machine_wanted:
        refuse(f"ELF machine {machine}, expected {machine_wanted} ({name})")
    print(f"library: ELF {name}")

def macho_version(v):
    return f"{v >> 16}.{(v >> 8) & 0xFF}.{v & 0xFF}"

def macho():
    magic = data[:4]
    if magic in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf", b"\xbe\xba\xfe\xca", b"\xbf\xba\xfe\xca"):
        refuse("a universal binary, and the tag names one architecture")
    if magic != b"\xcf\xfa\xed\xfe":
        refuse("not a 64-bit little-endian Mach-O library")
    cputype, _, _, ncmds = struct.unpack_from("<iiII", data, 4)
    if cputype != 0x0100000C:
        refuse(f"Mach-O cputype {cputype:#x}, expected arm64 (0x100000c)")
    offset, minos = 32, None
    for _ in range(ncmds):
        cmd, size = struct.unpack_from("<II", data, offset)
        if size < 8:
            refuse("a load command with an impossible size")
        if cmd == 0x32:  # LC_BUILD_VERSION
            platform_id, version = struct.unpack_from("<II", data, offset + 8)
            if platform_id != 1:  # PLATFORM_MACOS
                refuse(f"built for Mach-O platform {platform_id}, not macOS")
            minos = version
        elif cmd == 0x24:  # LC_VERSION_MIN_MACOSX
            minos = struct.unpack_from("<I", data, offset + 8)[0]
        offset += size
    if minos is None:
        refuse("no LC_BUILD_VERSION or LC_VERSION_MIN_MACOSX: its minimum macOS is unknown")
    major, _, minor = floor.partition(".")
    floor_version = (int(major) << 16) | (int(minor or 0) << 8)
    if minos > floor_version:
        refuse(f"needs macOS {macho_version(minos)}, newer than the {floor} the wheel claims")
    print(f"library: Mach-O arm64, minimum macOS {macho_version(minos)} (claimed {floor})")

def pe():
    if data[:2] != b"MZ" or len(data) < 0x40:
        refuse("not a PE library")
    header = struct.unpack_from("<I", data, 0x3C)[0]
    if data[header:header + 4] != b"PE\0\0":
        refuse("not a PE library")
    machine = struct.unpack_from("<H", data, header + 4)[0]
    if machine != 0x8664:
        refuse(f"PE machine {machine:#06x}, expected x86_64 (0x8664)")
    print("library: PE x86_64")

try:
    {
        "linux-x86_64": lambda: elf(62, "x86_64"),
        "linux-aarch64": lambda: elf(183, "aarch64"),
        "macos-arm64": macho,
        "windows-x86_64": pe,
    }.get(platform, lambda: refuse(f"unknown platform {platform}"))()
except struct.error:
    refuse("truncated header")
PYTHON

case "$PLATFORM" in
  macos-arm64)
    LOADER_NAME=libuniffi.dylib
    PLAT_TAG="macosx_${MACOS_FLOOR//./_}_arm64"
    ;;
  linux-x86_64)
    LOADER_NAME=libuniffi.so
    PLAT_TAG="$(manylinux_tag x86_64)"
    ;;
  linux-aarch64)
    LOADER_NAME=libuniffi.so
    PLAT_TAG="$(manylinux_tag aarch64)"
    ;;
  windows-x86_64)
    LOADER_NAME=uniffi.dll
    PLAT_TAG=win_amd64
    ;;
  *)
    die "unknown platform: $PLATFORM"
    ;;
esac

# One copy, under the one name the generated loader opens. The cargo name is
# loaded by nothing, and package-data would otherwise ship both.
rm -f "$PKG_DIR"/*.dylib "$PKG_DIR"/*.so "$PKG_DIR"/*.dll
cp "$LIBRARY" "$PKG_DIR/$LOADER_NAME"

# Exactly one version line, or the substitution below is a guess.
[ "$(grep -c '^version = "' "$PY_ROOT/pyproject.toml")" = 1 ] ||
  die "pyproject.toml must have exactly one top-level version line"
sed -i.bak "s/^version = \".*\"/version = \"$PY_VERSION\"/" "$PY_ROOT/pyproject.toml"
rm -f "$PY_ROOT/pyproject.toml.bak"

rm -rf "$PY_ROOT/dist" "$PY_ROOT/build"
(cd "$PY_ROOT" && python -m build --wheel >&2)

BUILT=("$PY_ROOT"/dist/*.whl)
[ "${#BUILT[@]}" = 1 ] && [ -f "${BUILT[0]}" ] || die "expected one wheel in dist/, found ${#BUILT[*]}"

python -m wheel tags --remove --python-tag py3 --abi-tag none \
  --platform-tag "$PLAT_TAG" "${BUILT[0]}" >&2

WHEEL="$PY_ROOT/dist/offline_protocol_sdk-$PY_VERSION-py3-none-$PLAT_TAG.whl"
[ -f "$WHEEL" ] || die "the retagged wheel is not where it should be: $WHEEL"
[ "$(find "$PY_ROOT/dist" -name '*.whl' | wc -l | tr -d ' ')" = 1 ] ||
  die "dist/ holds more than the retagged wheel"

# What the wheel has to carry: the library under the loader's name, the
# bindings that open it, and the licences.
python - "$WHEEL" "$LOADER_NAME" <<'PYTHON'
import sys, zipfile
wheel, loader = sys.argv[1], sys.argv[2]
names = set(zipfile.ZipFile(wheel).namelist())
required = [f"offline_protocol_sdk/{loader}", "offline_protocol_sdk/offline_protocol.py"]
missing = [n for n in required if n not in names]
libraries = [n for n in names if n.endswith((".so", ".dylib", ".dll"))]
licences = [n for n in names if n.endswith(("/LICENSE", "/EXPORT.md"))]
assert not missing, f"the wheel is missing {missing}"
assert libraries == [f"offline_protocol_sdk/{loader}"], f"the wheel carries {libraries}"
assert len(licences) >= 2, f"the wheel carries no licence or export notice: {licences}"
PYTHON

echo "$WHEEL"
