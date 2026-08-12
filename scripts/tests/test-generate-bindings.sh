#!/usr/bin/env bash

# Guards the single-entry-point rule for UniFFI codegen.
#
# scripts/generate-bindings.sh exists so that Swift, Kotlin and Python are
# always regenerated together — they are one artifact set off one UDL, and a
# partial set does not fail any build, only the app at its first FFI call. That
# property is only as good as two things nothing else checks: that the
# generator still writes all three languages to the three committed paths, and
# that nothing else in the repo shells out to uniffi-bindgen behind its back.
#
# Both are asserted here, and deliberately by *behavior* rather than by
# grepping the generator's source. A guard that greps for `generate swift`
# fails a correct refactor into a loop and passes a one-character typo in an
# output path — the second being the exact bug class this whole rule exists to
# prevent. So the generator is instead run for real against a stub
# uniffi-bindgen, and what it asked the bindgen to do is checked. Still no real
# bindgen, no Rust toolchain, no network, well under a second: cheap enough to
# be the first step of the CI job, which is where it earns its keep.
#
# Scope of the scan is every tracked file, matched by content. Not `*.sh`:
# release.yml's Kotlin step generates into a scratch directory that the publish
# job downloads over the committed Android bindings, so a direct invocation in
# a workflow ships to npm without touching a shell script — and the same goes
# for a composite action, a package.json script, a Gradle exec task, or a doc
# that simply tells a human to run bindgen. Extension-scoped scanning would
# call this rule enforced while leaving those exempt.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GENERATOR="scripts/generate-bindings.sh"
SELF="scripts/tests/test-generate-bindings.sh"

cd "$REPO_ROOT"

# A direct invocation, in every shape it gets written in: subcommand adjacent
# (`uniffi-bindgen generate`), behind global options (`uniffi-bindgen --config
# x.toml generate`), continued onto the next line (`uniffi-bindgen \`), or via
# cargo (`cargo run --bin uniffi-bindgen -- generate`). Only flag-shaped tokens
# and their values may sit between the binary and the subcommand, which is what
# keeps prose like "run uniffi-bindgen to generate the bindings" from matching
# while the command forms all do.
INVOCATION='(^|[^[:alnum:]_-])uniffi-bindgen([[:space:]]+-{1,2}[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+generate|(^|[^[:alnum:]_-])uniffi-bindgen[[:space:]]*\\[[:space:]]*$|--bin[[:space:]]+uniffi-bindgen'

# A line that actually *runs* the generator, as opposed to merely naming it.
# Anchored at the start of the line (allowing a YAML `run:` prefix) so that a
# comment pointing at the script, or an echo printing it in an error message,
# does not count as delegating — those are exactly what is left behind when
# someone deletes the real call.
# The trailing path-separator requirement matters: without it this also matches
# `bash scripts/tests/test-generate-bindings.sh`, so a workflow that runs only
# the guard would read as delegating to the generator it is supposed to guard.
DELEGATION='^[[:space:]]*(run:[[:space:]]+)?(exec[[:space:]]+)?bash[[:space:]]+([^|;&]*[/"])?generate-bindings\.sh'

# Every caller the generator's header claims delegates to it. Listed here so
# that deleting a delegation is a test failure rather than a silent return to
# per-language generation from the other side.
DELEGATING_CALLERS="
bindings/react-native/scripts/generate-bindings.sh
bindings/react-native/scripts/build-uniffi-ios.sh
bindings/react-native/scripts/build-uniffi-android.sh
bindings/python/scripts/build-desktop.sh
.github/workflows/ci.yml
.github/workflows/release.yml
"

# The three committed paths, repo-relative. The drift gate in ci.yml checks
# exactly these; if the generator ever writes somewhere else, `git diff` on a
# path nothing wrote to reports no diff and the gate goes green on stale
# bindings. This list is what makes that detectable.
EXPECTED_OUTPUTS="
swift:bindings/react-native/ios/Generated
kotlin:bindings/react-native/android/src/main/java
python:bindings/python/offline_protocol_sdk
"

FAILURES=0

pass() { echo "  ok — $*"; }

fail() {
  echo "  FAIL — $*" >&2
  FAILURES=$((FAILURES + 1))
}

echo "Guarding the shared binding generator..."

# ---------------------------------------------------------------------------
# 1. The generator exists and is executable.
# ---------------------------------------------------------------------------

if [[ -x "$GENERATOR" ]]; then
  pass "$GENERATOR is executable"
else
  fail "$GENERATOR is missing or not executable"
fi

# ---------------------------------------------------------------------------
# 2. Behavioral: run the real generator against a stub bindgen and check what
#    it actually asked for — languages, output paths, and --no-format.
# ---------------------------------------------------------------------------

STUB_ROOT="$(mktemp -d)"
trap 'rm -rf "$STUB_ROOT"' EXIT

# Reports a patch version deliberately different from the crate pin, so the
# run also proves the version gate tolerates patch drift (uniffi's FFI contract
# is not patch-scoped) rather than only that it exists.
make_stub() {
  local bin_dir="$1" reported="$2"
  mkdir -p "$bin_dir"
  cat >"$bin_dir/uniffi-bindgen" <<STUB
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then
  echo "uniffi-bindgen $reported"
  exit 0
fi
printf '%s\n' "\$*" >>"\$STUB_LOG"
exit 0
STUB
  chmod +x "$bin_dir/uniffi-bindgen"
}

CRATE_PIN="$(sed -n 's/^uniffi = "\([0-9.]*\)"$/\1/p' crates/offline-protocol-uniffi/Cargo.toml | head -1)"
if [[ -z "$CRATE_PIN" ]]; then
  fail "could not read the uniffi pin from crates/offline-protocol-uniffi/Cargo.toml"
  CRATE_PIN="0.0"
fi

ok_bin="$STUB_ROOT/ok"
call_log="$STUB_ROOT/calls.log"
: >"$call_log"
make_stub "$ok_bin" "$CRATE_PIN.99"

if PATH="$ok_bin:$PATH" STUB_LOG="$call_log" bash "$GENERATOR" >"$STUB_ROOT/out" 2>&1; then
  pass "$GENERATOR runs to completion (patch-level version drift tolerated)"
else
  fail "$GENERATOR failed against a stub bindgen: $(tail -3 "$STUB_ROOT/out" | tr '\n' ' ')"
fi

call_count="$(grep -c . "$call_log" || true)"
if [[ "$call_count" -eq 3 ]]; then
  pass "it invoked bindgen exactly 3 times — one per language"
else
  fail "it invoked bindgen $call_count time(s), expected 3 (Swift, Kotlin, Python)"
fi

while IFS= read -r spec; do
  [[ -z "$spec" ]] && continue
  language="${spec%%:*}"
  expected_dir="${spec#*:}"
  line="$(grep -- "--language $language " "$call_log" || true)"
  if [[ -z "$line" ]]; then
    fail "$GENERATOR never generated $language — a caller would ship a partial set"
    continue
  fi
  actual_dir="${line##*--out-dir }"
  actual_dir="${actual_dir%% *}"
  actual_dir="${actual_dir#"$REPO_ROOT"/}"
  if [[ "$actual_dir" == "$expected_dir" ]]; then
    pass "$language -> $expected_dir"
  else
    fail "$language written to '$actual_dir', but the drift gate checks '$expected_dir' — a mismatch here is invisible to CI (git diff on a path nothing wrote to reports no diff)"
    # mkdir -p in the generator may have created the wrong path; take it back
    # if it is empty so a failing run does not litter the tree.
    rmdir "$actual_dir" 2>/dev/null || true
  fi
done <<EOF
$EXPECTED_OUTPUTS
EOF

# --no-format is load-bearing: uniffi-bindgen post-processes with ktlint and
# swiftformat when they are on PATH, so without it the committed bytes depend
# on what the developer happens to have installed and a correct regeneration
# can still fail the drift gate.
unformatted="$(grep -c -- '--no-format' "$call_log" || true)"
if [[ "$unformatted" -eq "$call_count" && "$call_count" -gt 0 ]]; then
  pass "every invocation passes --no-format, so output does not vary by machine"
else
  fail "only $unformatted of $call_count invocations pass --no-format — output would depend on whether ktlint/swiftformat are installed"
fi

# ---------------------------------------------------------------------------
# 3. Behavioral: the version gate actually rejects a mismatched bindgen.
# ---------------------------------------------------------------------------

bad_bin="$STUB_ROOT/bad"
bad_major="${CRATE_PIN%%.*}"
bad_minor="${CRATE_PIN#*.}"
make_stub "$bad_bin" "$bad_major.$((bad_minor + 1)).0"

if PATH="$bad_bin:$PATH" STUB_LOG="$STUB_ROOT/bad.log" bash "$GENERATOR" >/dev/null 2>&1; then
  fail "$GENERATOR generated with a bindgen whose minor version disagrees with the crate pin — those bindings' checksums fail at runtime, not at build time"
else
  pass "a bindgen disagreeing with the crate pin is rejected before anything is written"
fi

# ---------------------------------------------------------------------------
# 4. Nothing else invokes uniffi-bindgen — any tracked file, matched by
#    content. Excluded by pathspec rather than by comparing "$0" against the
#    index, so the result cannot depend on how this script was invoked.
# ---------------------------------------------------------------------------

set +e
offenders="$(git grep -lIE "$INVOCATION" -- . ":!$GENERATOR" ":!$SELF")"
scan_rc=$?
set -e

case "$scan_rc" in
  0)
    fail "these files invoke uniffi-bindgen directly instead of delegating to $GENERATOR:"
    printf '%s\n' "$offenders" | sed 's/^/      /' >&2
    ;;
  1) pass "no other tracked file invokes uniffi-bindgen directly" ;;
  *) fail "the scan could not run (git grep exit $scan_rc) — it would otherwise report success having examined nothing" ;;
esac

# Positive control on the *real* corpus. Every other assertion here is "the bad
# thing is absent", which is exactly what a scan that silently examines zero
# files also reports. Re-run without the generator exclusion: it must find the
# generator, or enumeration is broken and the check above proved nothing.
if git grep -lIE "$INVOCATION" -- . ":!$SELF" | grep -qx "$GENERATOR"; then
  pass "the scan reaches the repository (it finds $GENERATOR when not excluded)"
else
  fail "the scan did not find $GENERATOR — enumeration is broken, so the offender check above is vacuous"
fi

# ---------------------------------------------------------------------------
# 5. The enumerated callers still delegate. Without this, deleting the
#    delegation from a build script leaves the guard green while that script
#    builds a fresh native library against stale committed bindings.
# ---------------------------------------------------------------------------

while IFS= read -r caller; do
  [[ -z "$caller" ]] && continue
  if [[ ! -f "$caller" ]]; then
    fail "$caller is missing — the generator's header still lists it as delegating"
  elif grep -Eq "$DELEGATION" "$caller"; then
    pass "$caller invokes $GENERATOR"
  else
    fail "$caller no longer invokes $GENERATOR — it can now build artifacts against stale bindings"
  fi
done <<EOF
$DELEGATING_CALLERS
EOF

# ---------------------------------------------------------------------------
# 6. Negative controls for the scan regex. A detector that cannot fire is
#    indistinguishable from one that passes vacuously, so prove it catches
#    every invocation shape and ignores the benign ones.
# ---------------------------------------------------------------------------

fixtures="$STUB_ROOT/fixtures"
mkdir -p "$fixtures"

printf 'uniffi-bindgen generate x.udl --language swift\n'          >"$fixtures/hit-inline"
printf 'uniffi-bindgen \\\n  generate x.udl --language kotlin\n'   >"$fixtures/hit-continued"
printf 'uniffi-bindgen --config u.toml generate x.udl\n'           >"$fixtures/hit-global-option"
printf 'cargo run --bin uniffi-bindgen -- generate x.udl\n'        >"$fixtures/hit-cargo-run"
printf 'if command -v uniffi-bindgen; then bash %s; fi\n' "$GENERATOR" >"$fixtures/miss-delegating"
printf 'Run uniffi-bindgen to generate the Swift bindings.\n'      >"$fixtures/miss-prose"

for fixture in "$fixtures"/hit-*; do
  if grep -Eq "$INVOCATION" "$fixture"; then
    pass "the scan detects $(basename "$fixture" | sed 's/^hit-//') invocations"
  else
    fail "the scan MISSES $(basename "$fixture" | sed 's/^hit-//') invocations — it would pass vacuously"
  fi
done

for fixture in "$fixtures"/miss-*; do
  if grep -Eq "$INVOCATION" "$fixture"; then
    fail "the scan falsely flags $(basename "$fixture" | sed 's/^miss-//')"
  else
    pass "the scan ignores $(basename "$fixture" | sed 's/^miss-//')"
  fi
done

echo ""
if [[ "$FAILURES" -eq 0 ]]; then
  echo "All binding-generator guards passed."
else
  echo "$FAILURES guard(s) failed." >&2
  exit 1
fi
