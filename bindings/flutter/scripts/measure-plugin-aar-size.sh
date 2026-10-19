#!/usr/bin/env bash
#
# Measures the Flutter plugin release AAR (PR3 native surface + jniLibs).
# CI uses this single Gradle invocation (:offline_protocol:assembleRelease) to
# compile shared Kotlin and gate footprint; no separate compileDebugKotlin step.
#
#   bindings/flutter/scripts/measure-plugin-aar-size.sh           # print size
#   bindings/flutter/scripts/measure-plugin-aar-size.sh --check   # fail if over baseline + tolerance
#   bindings/flutter/scripts/measure-plugin-aar-size.sh --write-baseline
#
# Requires: JDK 17+, example android/gradlew (Flutter example project).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BASELINE="${REPO_ROOT}/bindings/flutter/baselines/offline-protocol-release-aar.json"
ANDROID_DIR="${REPO_ROOT}/bindings/flutter/offline_protocol/example/android"
AAR_NAME="offline_protocol-release.aar"

MODE="${1:-}"

if [[ ! -x "${ANDROID_DIR}/gradlew" ]]; then
  echo "missing ${ANDROID_DIR}/gradlew — bootstrap the Flutter example first" >&2
  exit 1
fi

JNI_DIR="${REPO_ROOT}/bindings/react-native/android/src/main/jniLibs"
if ! compgen -G "${JNI_DIR}/arm64-v8a/*.so" >/dev/null 2>&1; then
  echo "UniFFI jniLibs not found under ${JNI_DIR}. Build React Native artifacts first:" >&2
  echo "  cd bindings/react-native && npm run build:uniffi:android" >&2
  exit 1
fi

cd "${ANDROID_DIR}"
./gradlew :offline_protocol:assembleRelease --quiet

AAR_PATH="$(find "${REPO_ROOT}/bindings/flutter/offline_protocol/example/build" -name "${AAR_NAME}" -type f 2>/dev/null | head -1)"
if [[ -z "${AAR_PATH}" || ! -f "${AAR_PATH}" ]]; then
  echo "could not find ${AAR_NAME} under example/build" >&2
  exit 1
fi

BYTES="$(wc -c < "${AAR_PATH}" | tr -d ' ')"
KIB="$(awk -v b="${BYTES}" 'BEGIN { printf "%.1f", b / 1024 }')"
MIB="$(awk -v b="${BYTES}" 'BEGIN { printf "%.2f", b / 1024 / 1024 }')"

case "${MODE}" in
  --write-baseline)
    if ! command -v python3 >/dev/null 2>&1; then
      echo "python3 required to update JSON baseline" >&2
      exit 1
    fi
    python3 - "${BASELINE}" "${BYTES}" <<'PY'
import json, sys
path, nbytes = sys.argv[1], int(sys.argv[2])
with open(path, encoding="utf-8") as f:
    data = json.load(f)
data["bytes"] = nbytes
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
print(f"updated {path} -> {nbytes} bytes")
PY
    ;;
  --check)
    if ! command -v python3 >/dev/null 2>&1; then
      echo "python3 required for --check" >&2
      exit 1
    fi
    python3 - "${BASELINE}" "${BYTES}" <<'PY'
import json, sys
path, measured = sys.argv[1], int(sys.argv[2])
with open(path, encoding="utf-8") as f:
    data = json.load(f)
expected = int(data["bytes"])
tol = int(data.get("tolerance_bytes", 0))
ceiling = expected + tol
if measured > ceiling:
    print(
        f"AAR size regression: {measured} bytes > baseline {expected} + tolerance {tol} (= {ceiling})",
        file=sys.stderr,
    )
    sys.exit(1)
print(f"OK: {measured} bytes within baseline {expected} + tolerance {tol}")
PY
    ;;
  "")
    ;;
  *)
    echo "usage: $0 [--check|--write-baseline]" >&2
    exit 1
    ;;
esac

printf '| Artifact | Bytes | KiB | MiB |\n'
printf '|---|--:|--:|--:|\n'
printf '| `%s` | %'"'"'d | %s | %s |\n' "${AAR_NAME}" "${BYTES}" "${KIB}" "${MIB}"
printf '\nPath: `%s`\n' "${AAR_PATH#${REPO_ROOT}/}"
