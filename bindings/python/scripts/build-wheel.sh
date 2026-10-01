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
# The Linux tag is read from the library, never written down. A manylinux tag
# promises the glibc floor it names; the library carries the floor of the image
# that built it, and a tag older than that installs on a system where the
# library then fails to load. The script also refuses a library that links
# anything outside the manylinux set, which no system can be assumed to have.
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

# manylinux_2_<N>_<arch>, N the newest GLIBC_2.N symbol version the library
# asks for.
manylinux_tag() {
  local arch="$1" needed newest
  command -v readelf >/dev/null || die "readelf is needed to read the library's glibc floor"

  while IFS= read -r needed; do
    local allowed=0 lib
    for lib in "${MANYLINUX_LIBS[@]}"; do
      [ "$needed" = "$lib" ] && allowed=1
    done
    [ "$allowed" = 1 ] || die "the library links $needed, which is outside the manylinux set"
  done < <(readelf -d -W "$LIBRARY" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')

  newest="$(readelf --dyn-syms -W "$LIBRARY" | grep -o 'GLIBC_2\.[0-9]*' | sort -t. -k2 -n -u | tail -1 || true)"
  [ -n "$newest" ] || die "no GLIBC_2.N symbol version in the library: is it a glibc build?"
  echo "manylinux_2_${newest#GLIBC_2.}_$arch"
}

case "$PLATFORM" in
  macos-arm64)
    LOADER_NAME=libuniffi.dylib
    # The library is built on macOS 14 with no deployment target set, so 14
    # is a floor it is known to meet. Claiming an older one would need a
    # target set at build time and checked here.
    PLAT_TAG=macosx_14_0_arm64
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
