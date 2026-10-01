#!/usr/bin/env bash

# Drives bindings/python/scripts/build-wheel.sh on a copy of the Python
# package, with a shared library compiled here whose glibc floor is known.
#
# The Linux platform tag is the one claim about a wheel nothing downstream
# re-checks: a tag older than the library's real floor installs on a system
# where the library then fails to load, and pip on the release's test runner,
# which is newer, accepts it. So the derivation is pinned against a library
# that asks for getrandom (GLIBC_2.25) and nothing newer, and a library that
# links outside the manylinux set must be refused. Needs gcc, readelf and
# Python with `build` and `wheel`: a Linux runner.

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

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "All wheel checks passed."
else
  echo "$FAILURES wheel check(s) failed." >&2
  exit 1
fi
