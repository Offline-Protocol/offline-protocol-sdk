#!/usr/bin/env bash
# Fail when a committed output of the shared engine-bridge generators is stale
# (docs/bridges/README.md, C9b). Each tool regenerates its output when run
# without --check; this script only ever checks.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" != "--check" ]]; then
  echo "usage: $0 --check" >&2
  exit 2
fi

failures=0
check() {
  if ! python3 "$@"; then
    failures=$((failures + 1))
  fi
}

check tools/gen_rn_protocol_engine_bridge_react_native.py --check
check tools/gen_rn_protocol_engine_bridge_react_native_swift.py --check
check tools/gen_protocol_engine_host_bridge.py --check
# The Swift bridge is hand-written: one handler per shared method, no stubs.
check tools/check_swift_protocol_engine_host_bridge.py
check tools/check_rn_adapter_argument_keys.py
check tools/gen_mesh_sdk_dart.py --check
check tools/gen_engine_method_handlers.py --check
check tools/gen_flutter_engine_bridge.py --check
check tools/gen_flutter_swift_bridge.py --check
check tools/gen_csharp_local_api.py --check
if [[ ! -f tools/rn_engine_bridge_error_manifest.json ]]; then
  echo "missing tools/rn_engine_bridge_error_manifest.json; run tools/extract_rn_bridge_error_manifest.py" >&2
  failures=$((failures + 1))
fi

if (( failures > 0 )); then
  exit 1
fi
echo "binding generators: OK"
