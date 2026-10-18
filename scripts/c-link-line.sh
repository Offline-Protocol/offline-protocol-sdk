#!/usr/bin/env bash
# Prints the command that links a C program (app.c) against a desktop
# release archive's library, run from inside the extracted archive.
#
#   c-link-line.sh PLATFORM     (macos-arm64, linux-x86_64, windows-x86_64, ...)
#
# One source for two readers: package-release-assets.sh writes it into each
# archive's README, and the release's check of the shipped libraries runs
# it verbatim. A line that links and then cannot load (a macOS library found
# through an rpath its install name never consults, say) fails the release
# rather than the first reader to copy it.
set -euo pipefail

case "${1:?platform}" in
  windows-*)
    # MSVC links a DLL through its import library; the DLL is found beside
    # the executable at run time.
    echo 'cl /I. app.c offline_protocol_uniffi.dll.lib'
    ;;
  macos-*)
    echo 'cc -I. app.c -L. -loffline_protocol_uniffi -Wl,-rpath,@loader_path'
    ;;
  linux-*)
    # shellcheck disable=SC2016 # $ORIGIN is for the linker, not the shell.
    echo 'cc -I. app.c -L. -loffline_protocol_uniffi -Wl,-rpath,'"'"'$ORIGIN'"'"
    ;;
  *)
    echo "no C link line for platform $1" >&2
    exit 1
    ;;
esac
