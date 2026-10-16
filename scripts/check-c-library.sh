#!/usr/bin/env bash
# Builds and runs the C quick start against a desktop library the way the
# release archive's README says to: the library and the header beside
# app.c, linked with the archive's own line (c-link-line.sh), and then run
# from another directory, after the directory has been moved, so the program
# can only find the library the way a reader's would.
#
#   check-c-library.sh PLATFORM LIBDIR
#
# LIBDIR holds the library as a desktop build job collects it (and, on
# Windows, its import library). The release runs this against the bytes it
# ships; CI runs it on Windows against a debug build, where it is the only
# C compiled against the library.
set -euo pipefail

plat="${1:?platform}"
libdir="${2:?library directory}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"

case "$plat" in
  windows-*) files=(offline_protocol_uniffi.dll offline_protocol_uniffi.dll.lib); program=app.exe ;;
  macos-*) files=(liboffline_protocol_uniffi.dylib); program=a.out ;;
  linux-*) files=(liboffline_protocol_uniffi.so); program=a.out ;;
  *) echo "unhandled platform $plat" >&2; exit 1 ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/app" "$work/data"
for file in "${files[@]}"; do
  if [ -f "$libdir/$file" ]; then
    cp "$libdir/$file" "$work/app/"
  elif [ -f "$libdir/deps/$file" ]; then
    cp "$libdir/deps/$file" "$work/app/"
  else
    echo "$file is not in $libdir" >&2
    exit 1
  fi
done
cp "$root/bindings/c/include/offline_protocol.h" "$work/app/"

data="$work/data"
if command -v cygpath >/dev/null 2>&1; then
  # A Windows path with forward slashes: no escaping inside the JSON.
  data="$(cygpath -m "$data")"
fi
"$root/bindings/c/consumer-check/quickstart.sh" "$root/bindings/c/README.md" "$work/app/app.c" "$data"

line="$(bash "$root/scripts/c-link-line.sh" "$plat")"
echo "+ $line"
# MSYS would read cl's /I. as a path and rewrite it.
(cd "$work/app" && export MSYS2_ARG_CONV_EXCL='*' && eval "$line")

mv "$work/app" "$work/moved"
out="$(cd / && "$work/moved/$program")"
echo "$out"
case "$out" in
  *'this device is "off1'*) echo "c library check ($plat): ok" ;;
  *) echo "the quick start did not report an address" >&2; exit 1 ;;
esac
