#!/usr/bin/env bash
# Fail when committed bridge generator outputs are stale (run without --check to regen).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

check=0
if [[ "${1:-}" == "--check" ]]; then
  check=1
fi

run() {
  if (( check )); then
    "$@"
  else
    "$@" 2>/dev/null || true
  fi
}

failures=0

if (( check )); then
  python3 tools/gen_rn_protocol_engine_bridge_react_native.py --check || failures=$((failures + 1))
  python3 tools/gen_rn_protocol_engine_bridge_react_native_swift.py --check || failures=$((failures + 1))
  python3 tools/gen_swift_protocol_engine_host_bridge.py || failures=$((failures + 1))
  python3 tools/gen_protocol_engine_host_bridge.py --check || failures=$((failures + 1))
  python3 tools/gen_mesh_sdk_dart.py --check || failures=$((failures + 1))
  if [[ ! -f tools/rn_engine_bridge_error_manifest.json ]]; then
    echo "missing tools/rn_engine_bridge_error_manifest.json; run tools/extract_rn_bridge_error_manifest.py" >&2
    failures=$((failures + 1))
  fi
else
  echo "usage: $0 --check" >&2
  exit 2
fi

if (( failures > 0 )); then
  exit 1
fi
echo "binding generators: OK"
