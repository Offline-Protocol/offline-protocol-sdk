#!/usr/bin/env bash

# `npm run build:android` — kept as the documented entry point, but the work
# lives in build-uniffi-android.sh.
#
# This used to be a near-copy of that script minus the binding generation, and
# that made it the largest remaining hole in the single-entry-point rule: it
# produced fresh .so files beside whatever Kotlin happened to be committed,
# which is the ABI mismatch scripts/generate-bindings.sh exists to prevent —
# not on a missing-bindgen edge case, but on every run.
#
# The copy had also drifted behind on two things that matter more than the
# duplication: it never ran check-elf-alignment.py, so it produced libraries
# Google Play rejects for 16 KB page alignment (release.yml enforces this on
# every ABI), and its NDK toolchain resolution predates the modern prebuilt
# layouts build-uniffi-android.sh handles. Delegating fixes all three at once.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec bash "$SCRIPT_DIR/build-uniffi-android.sh" "$@"
