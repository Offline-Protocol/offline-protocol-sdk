#!/usr/bin/env bash
# Writes the quick start in bindings/c/README.md (its first ```c block) to
# OUT, as a reader copies it. Given ROOT, fills in the program's two
# placeholders, the storage paths and the store key, which is the one change
# a reader must make before it runs. Used by the consumer check's Makefile
# and by the release's check of the shipped libraries, so both build the
# example the README shows, not a copy of it.
#
#   quickstart.sh README OUT [ROOT]
set -euo pipefail

readme="${1:?README}"
out="${2:?OUT}"
root="${3:-}"

awk '/^```c$/{inside=1; next} inside && /^```$/{exit} inside' "$readme" >"$out"
grep -q 'offline_protocol_create' "$out" || {
  echo "no quick start found in $readme" >&2
  exit 1
}
if [ -n "$root" ]; then
  # A fixed key: this program's stores live for one run.
  sed -e "s|/var/lib/app|$root|g" \
    -e 's|<32 bytes, base64>|AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=|' \
    "$out" >"$out.tmp"
  mv "$out.tmp" "$out"
  if grep -q '/var/lib/app\|<32 bytes' "$out"; then
    echo "a placeholder in the quick start was not filled in" >&2
    exit 1
  fi
fi
