#!/usr/bin/env bash
#
# Builds the arm64-v8a UniFFI shared library for Flutter CI and local checks.
# One ABI keeps pull-request jobs fast; release still builds all ABIs. The
# profile is `mobile`, the one the Android release ships (ADR 0029), so the
# footprint baseline measures the library applications actually get.
#
# Requires: ANDROID_NDK_HOME, Rust aarch64-linux-android target.
# Writes: bindings/react-native/android/src/main/jniLibs/arm64-v8a/libuniffi_offline_protocol.so

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
JNI_OUT="${REPO_ROOT}/bindings/react-native/android/src/main/jniLibs/arm64-v8a"
TARGET=aarch64-linux-android
LINKER_BIN=aarch64-linux-android21-clang

if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  echo "ANDROID_NDK_HOME is not set" >&2
  exit 1
fi

NDK_TOOLCHAIN="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/linux-x86_64/bin"
if [[ ! -d "$NDK_TOOLCHAIN" ]]; then
  # macOS local runs (CI uses linux-x86_64).
  NDK_TOOLCHAIN="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/darwin-x86_64/bin"
fi
if [[ ! -x "${NDK_TOOLCHAIN}/${LINKER_BIN}" ]]; then
  echo "NDK clang not found at ${NDK_TOOLCHAIN}/${LINKER_BIN}" >&2
  exit 1
fi

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="${NDK_TOOLCHAIN}/${LINKER_BIN}"
export AR="${NDK_TOOLCHAIN}/llvm-ar"
export CC_aarch64_linux_android="${NDK_TOOLCHAIN}/${LINKER_BIN}"
export CXX_aarch64_linux_android="${NDK_TOOLCHAIN}/${LINKER_BIN}++"

cd "${REPO_ROOT}"
cargo build --profile mobile --locked --target "${TARGET}" \
  --package offline-protocol-uniffi --no-default-features

SO_ROOT="target/${TARGET}/mobile"
SO_PATH="${SO_ROOT}/liboffline_protocol_uniffi.so"
if [[ ! -f "${SO_PATH}" ]]; then
  SO_PATH="${SO_ROOT}/deps/liboffline_protocol_uniffi.so"
fi
if [[ ! -f "${SO_PATH}" ]]; then
  echo "liboffline_protocol_uniffi.so not found under ${SO_ROOT}" >&2
  exit 1
fi

mkdir -p "${JNI_OUT}"
cp "${SO_PATH}" "${JNI_OUT}/libuniffi_offline_protocol.so"
python3 bindings/react-native/scripts/check-elf-alignment.py \
  "${JNI_OUT}/libuniffi_offline_protocol.so"

echo "Installed ${JNI_OUT}/libuniffi_offline_protocol.so"
