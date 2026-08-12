#!/usr/bin/env bash

# Guards the single-entry-point rule for UniFFI codegen.
#
# scripts/generate-bindings.sh exists so that Swift, Kotlin and Python are
# always regenerated together — they are one artifact set off one UDL, and a
# partial set does not fail any build, only the app at its first FFI call. That
# property is only as good as the rule that nothing else shells out to
# uniffi-bindgen behind its back, and nothing about a new build script makes
# violating it obvious. So: assert it, statically, with no bindgen required.
#
# Out of scope by design: .github/workflows/release.yml's Kotlin step, which
# generates into an ephemeral release build directory rather than the committed
# paths this rule is about.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GENERATOR="scripts/generate-bindings.sh"

FAILURES=0

pass() { echo "  ok — $*"; }

fail() {
  echo "  FAIL — $*" >&2
  FAILURES=$((FAILURES + 1))
}

cd "$REPO_ROOT"

echo "Guarding the shared binding generator..."

# 1. The generator exists, is executable, and covers all three languages.
if [[ -x "$GENERATOR" ]]; then
  pass "$GENERATOR is executable"
else
  fail "$GENERATOR is missing or not executable"
fi

for language in swift kotlin python; do
  if grep -q "^generate $language " "$GENERATOR"; then
    pass "$GENERATOR generates $language"
  else
    fail "$GENERATOR no longer generates $language — a caller would silently ship a partial set"
  fi
done

# 2. No other tracked shell script invokes uniffi-bindgen directly. Callers must
#    delegate, or they can refresh one language and leave the other two stale.
offenders=""
while IFS= read -r script; do
  [[ "$script" == "$GENERATOR" ]] && continue
  if grep -q 'uniffi-bindgen generate' "$script"; then
    offenders+="      $script"$'\n'
  fi
done < <(git ls-files '*.sh')

if [[ -z "$offenders" ]]; then
  pass "no other shell script calls uniffi-bindgen generate directly"
else
  fail "these scripts call uniffi-bindgen generate instead of delegating to $GENERATOR:"
  printf '%s' "$offenders" >&2
fi

# 3. The version check reads the crate pin rather than repeating it, so the
#    pin has exactly one place of record.
if grep -q 'crates/offline-protocol-uniffi' "$GENERATOR" \
  && grep -q 'REQUIRED_VERSION' "$GENERATOR"; then
  pass "the bindgen version check is derived from the crate's uniffi pin"
else
  fail "$GENERATOR no longer derives the required bindgen version from the crate pin"
fi

echo ""
if [[ "$FAILURES" -eq 0 ]]; then
  echo "All binding-generator guards passed."
else
  echo "$FAILURES guard(s) failed." >&2
  exit 1
fi
