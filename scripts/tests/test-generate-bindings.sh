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
# Scope covers tracked shell scripts *and* the GitHub workflows. The workflows
# are not an afterthought: release.yml's Kotlin step generates into a scratch
# directory that the publish job then downloads over the committed Android
# bindings, so a direct invocation there ships to npm without ever touching a
# .sh file. A guard that read only *.sh would call that rule enforced while
# leaving the one path users actually receive exempt.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GENERATOR="scripts/generate-bindings.sh"

cd "$REPO_ROOT"

SELF="${BASH_SOURCE[0]}"
case "$SELF" in
  /*) SELF="${SELF#"$REPO_ROOT"/}" ;;
esac

# A direct invocation, in either of the two shapes it is written in: the command
# and subcommand on one line, or `uniffi-bindgen \` continued onto the next.
# Matching the flat string "uniffi-bindgen generate" alone would miss the
# continuation form — which is precisely how every script this rule replaced
# used to be written, so it is the likeliest way the violation comes back.
# Deliberately does not match `command -v uniffi-bindgen` or the install
# instructions that callers legitimately still print.
INVOCATION='(^|[^[:alnum:]_-])uniffi-bindgen([[:space:]]+generate|[[:space:]]*\\[[:space:]]*$)'

FAILURES=0

pass() { echo "  ok — $*"; }

fail() {
  echo "  FAIL — $*" >&2
  FAILURES=$((FAILURES + 1))
}

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

# 2. Nothing else invokes uniffi-bindgen directly — not a shell script, not a
#    workflow. Callers must delegate, or they can refresh one language and
#    leave the other two describing a different ABI.
offenders=""
while IFS= read -r file; do
  [[ "$file" == "$GENERATOR" || "$file" == "$SELF" ]] && continue
  if grep -Eq "$INVOCATION" "$file"; then
    offenders+="      $file"$'\n'
  fi
done < <(git ls-files '*.sh' '.github/workflows/*.yml')

if [[ -z "$offenders" ]]; then
  pass "no other script or workflow calls uniffi-bindgen directly"
else
  fail "these files call uniffi-bindgen directly instead of delegating to $GENERATOR:"
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

# 4. Codegen is pinned to one output form. uniffi-bindgen post-processes with
#    ktlint (Kotlin) and swiftformat (Swift) when they are on PATH, so without
#    --no-format the committed bytes depend on the developer's machine and a
#    correct regeneration can still fail the drift gate.
if grep -q 'uniffi-bindgen generate --no-format' "$GENERATOR"; then
  pass "$GENERATOR generates with --no-format, so output does not vary by machine"
else
  fail "$GENERATOR dropped --no-format — output now depends on whether ktlint/swiftformat are installed"
fi

# 5. Negative control. Every assertion above says "the good state is present",
#    and a detector that cannot fail is indistinguishable from one that passes
#    vacuously. Prove the scan in step 2 actually fires — on both invocation
#    shapes — against fixtures that are never part of the real scan.
control_dir="$(mktemp -d)"
trap 'rm -rf "$control_dir"' EXIT

printf '#!/usr/bin/env bash\nuniffi-bindgen generate src/x.udl --language swift\n' \
  >"$control_dir/inline.sh"
printf '#!/usr/bin/env bash\nuniffi-bindgen \\\n  generate src/x.udl --language kotlin\n' \
  >"$control_dir/continued.sh"
printf '#!/usr/bin/env bash\nif command -v uniffi-bindgen; then bash scripts/generate-bindings.sh; fi\n' \
  >"$control_dir/delegating.sh"

for shape in inline continued; do
  if grep -Eq "$INVOCATION" "$control_dir/$shape.sh"; then
    pass "the scan detects a $shape uniffi-bindgen invocation"
  else
    fail "the scan MISSES a $shape uniffi-bindgen invocation — it would pass vacuously"
  fi
done

if grep -Eq "$INVOCATION" "$control_dir/delegating.sh"; then
  fail "the scan flags a delegating caller that only probes for uniffi-bindgen"
else
  pass "the scan ignores a delegating caller that only probes for uniffi-bindgen"
fi

echo ""
if [[ "$FAILURES" -eq 0 ]]; then
  echo "All binding-generator guards passed."
else
  echo "$FAILURES guard(s) failed." >&2
  exit 1
fi
