#!/usr/bin/env bash
# Fail when committed RN engine-bridge generator outputs are stale.
# Flutter generator checks are added in a follow-up PR with the Flutter plugin.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

check=0
if [[ "${1:-}" == "--check" ]]; then
  check=1
fi

if (( ! check )); then
  echo "usage: $0 --check" >&2
  exit 2
fi

failures=0

python3 tools/gen_rn_protocol_engine_bridge_react_native.py --check || failures=$((failures + 1))
python3 tools/gen_rn_protocol_engine_bridge_react_native_swift.py --check || failures=$((failures + 1))
python3 tools/gen_protocol_engine_host_bridge.py --check || failures=$((failures + 1))

if [[ ! -f tools/rn_engine_bridge_error_manifest.json ]]; then
  echo "missing tools/rn_engine_bridge_error_manifest.json; run tools/extract_rn_bridge_error_manifest.py" >&2
  failures=$((failures + 1))
fi

if (( failures > 0 )); then
  exit 1
fi
echo "binding generators: OK"
