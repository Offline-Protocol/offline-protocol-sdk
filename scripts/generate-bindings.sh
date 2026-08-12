#!/usr/bin/env bash

# Generate every UniFFI binding from the one UDL: Swift, Kotlin and Python.
#
# This is the single entry point, and the reason it exists is that the three
# generated files are one artifact set, not three independent ones: they are
# produced by one bindgen from one UDL and carry the FFI checksums of the
# cdylib they were generated against. Regenerating a subset leaves the others
# describing a different ABI — which does not fail any build, only the app at
# the first call. Every caller that used to run `uniffi-bindgen` itself now
# delegates here, so no path can produce a partial set:
#
#   bindings/react-native/scripts/generate-bindings.sh   (npm run generate:bindings)
#   bindings/react-native/scripts/build-uniffi-ios.sh
#   bindings/react-native/scripts/build-uniffi-android.sh
#   bindings/python/scripts/build-desktop.sh
#   .github/workflows/ci.yml                             (via build-desktop.sh)
#   .github/workflows/release.yml                        (android-bindings, python-bindings)
#
# The release workflow matters most and is the least obvious: the Kotlin it
# generates is downloaded *over* the committed Android bindings in the publish
# job, so it — not the committed file — is what ships to npm. Anything that
# generates there must come through this script, or the released Kotlin and the
# released Swift can be built by different bindgens.
#
# Usage:
#   ./scripts/generate-bindings.sh
#
# Requires uniffi-bindgen matching the crate's `uniffi` pin; the check below
# reads that pin rather than repeating it.

set -euo pipefail

# Takes no arguments, and says so rather than ignoring them. The wrapper at
# bindings/react-native/scripts/generate-bindings.sh forwards "$@", so someone
# reaching for `npm run generate:bindings -- --language swift` gets told why
# that flag does not exist instead of watching all three regenerate anyway and
# concluding the flag worked.
if [[ $# -gt 0 ]]; then
  # Repo-relative rather than the basename: two tracked files are named
  # generate-bindings.sh, and the RN one execs this, so a basename cannot say
  # which of them rejected the flag.
  echo "Error: scripts/generate-bindings.sh takes no arguments (got: $*)." >&2
  echo "It generates Swift, Kotlin and Python together on purpose — they are one" >&2
  echo "artifact set off one UDL, and a partial set fails at runtime, not build time." >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
UNIFFI_DIR="$PROJECT_ROOT/crates/offline-protocol-uniffi"
UDL="src/offline_protocol.udl"

SWIFT_OUT="$PROJECT_ROOT/bindings/react-native/ios/Generated"
KOTLIN_OUT="$PROJECT_ROOT/bindings/react-native/android/src/main/java"
PYTHON_OUT="$PROJECT_ROOT/bindings/python/offline_protocol_sdk"

# The bindgen version is baked into the generated code as FFI checksums, so
# generating with a bindgen that disagrees with the crate's `uniffi` compiles
# fine and fails at runtime instead. Read the required version from the crate
# so there is exactly one version of record.
REQUIRED_VERSION="$(sed -n 's/^uniffi = "\([0-9.]*\)"$/\1/p' "$UNIFFI_DIR/Cargo.toml" | head -1)"
if [[ -z "$REQUIRED_VERSION" ]]; then
  echo "Error: could not read the uniffi version from $UNIFFI_DIR/Cargo.toml." >&2
  echo "The pin moved or changed shape — fix this check rather than skipping it." >&2
  exit 1
fi

if ! command -v uniffi-bindgen &>/dev/null; then
  echo "Error: uniffi-bindgen not found." >&2
  echo "" >&2
  echo "  cargo install uniffi --version $REQUIRED_VERSION --features cli --locked" >&2
  echo "" >&2
  exit 1
fi

# Compare at major.minor on *both* sides. The crate pin and the installed
# version are written to different precisions by convention — `uniffi = "0.30"`
# against a bindgen that reports `0.30.0` — so normalizing only the installed
# side makes a patch-level pin (`uniffi = "0.30.1"`) permanently unsatisfiable,
# killing all codegen with a message naming a version that cannot fix it.
INSTALLED_VERSION="$(uniffi-bindgen --version | awk '{print $NF}')"
INSTALLED_MAJOR_MINOR="$(printf '%s' "$INSTALLED_VERSION" | cut -d. -f1,2)"
REQUIRED_MAJOR_MINOR="$(printf '%s' "$REQUIRED_VERSION" | cut -d. -f1,2)"
if [[ "$INSTALLED_MAJOR_MINOR" != "$REQUIRED_MAJOR_MINOR" ]]; then
  echo "Error: uniffi-bindgen $INSTALLED_VERSION does not match the crate's uniffi $REQUIRED_VERSION pin." >&2
  echo "Generating with it would produce bindings whose checksums fail at runtime, not at build time." >&2
  echo "" >&2
  echo "  cargo install uniffi --version $REQUIRED_VERSION --features cli --locked --force" >&2
  echo "" >&2
  exit 1
fi

# --no-format is load-bearing, not tidiness: uniffi-bindgen shells out to
# ktlint for Kotlin and swiftformat for Swift *when they happen to be on PATH*,
# so without it the bytes written depend on what the developer has installed.
# The committed bindings are the unformatted form (CI runners carry neither
# tool), which means an Android or iOS contributor with those formatters —
# exactly the people editing this repo — would regenerate correctly and still
# trip the drift gate. Pin the output to the one form instead.
generate() {
  local language="$1" out_dir="$2"
  mkdir -p "$out_dir"
  uniffi-bindgen generate --no-format "$UDL" --language "$language" --out-dir "$out_dir"
  printf '  %-7s -> %s\n' "$language" "${out_dir#"$PROJECT_ROOT"/}"
}

cd "$UNIFFI_DIR"

echo "Generating UniFFI bindings from $UDL (uniffi-bindgen $INSTALLED_VERSION)"
generate swift "$SWIFT_OUT"
generate kotlin "$KOTLIN_OUT"
generate python "$PYTHON_OUT"

echo "All bindings generated. Commit all three together — a partial commit is drift."
