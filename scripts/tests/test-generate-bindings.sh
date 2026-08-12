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
#
# Standing rule for anything added below: an assertion whose *setup* can fail
# silently is worse than no assertion, because it reports success. Every check
# that depends on a fixture proves the fixture exists first.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GENERATOR="scripts/generate-bindings.sh"
SELF="scripts/tests/test-generate-bindings.sh"
CI_WORKFLOW=".github/workflows/ci.yml"

cd "$REPO_ROOT"

# A direct invocation, in every shape it gets written in: subcommand adjacent
# (`uniffi-bindgen generate`), behind global options (`uniffi-bindgen --config
# x.toml generate`), continued onto the next line (`uniffi-bindgen \`), via
# cargo (`cargo run --bin uniffi-bindgen -- generate`), or as an argument list
# rather than a command line — `commandLine("uniffi-bindgen", "generate", …)`
# in a Gradle task, `subprocess.run([...])`, `spawnSync('uniffi-bindgen',
# ['generate', …])`. The list forms matter because this is a Gradle-built
# Android repo, so a codegen task is a realistic place for the rule to be
# broken, and a command-line-only regex would call it covered.
#
# In the command-line form only flag-shaped tokens and their values may sit
# between the binary and the subcommand, which is what keeps prose like "run
# uniffi-bindgen to generate the bindings" from matching while every command
# form does.
SQ=\'
INVOCATION='(^|[^[:alnum:]_-])uniffi-bindgen([[:space:]]+-{1,2}[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+generate'
INVOCATION="$INVOCATION"'|(^|[^[:alnum:]_-])uniffi-bindgen[[:space:]]*\\[[:space:]]*$'
INVOCATION="$INVOCATION"'|--bin[[:space:]]+uniffi-bindgen'
INVOCATION="$INVOCATION|uniffi-bindgen[\"${SQ}]?[],[:space:]\"${SQ}[]*generate"

# A line that actually *runs* the generator, as opposed to merely naming it.
# The interpreter is optional because the script is executable and this PR's
# own docs teach `./scripts/generate-bindings.sh`; requiring a literal `bash `
# would report a caller refactored to the direct-exec form as no longer
# delegating. Anchored at the start of the line (allowing a YAML `run:` prefix)
# so a comment pointing at the script, or an echo printing it in an error
# message, does not count — those are exactly what is left behind when someone
# deletes the real call. The path-separator requirement keeps this from also
# matching `bash scripts/tests/test-generate-bindings.sh`, which would let a
# workflow that runs only the guard read as delegating to the generator.
#
# The path prefix must contain no whitespace and no quotes, and that is the
# part doing the work above. A permissive `[^|;&]*` prefix swallows a whole
# comment or echo body, so `# TODO restore: bash .../generate-bindings.sh`
# satisfied the check — commenting out the call, which is the most natural way
# this regression actually happens, read as still delegating. Requiring a
# single unbroken path token is what makes the sentence above true rather than
# aspirational.
#
# A bare relative path is *not* enough on its own: it must either carry an
# interpreter (`bash x`, `sh x`) or be explicitly rooted (`./x`, `/x`, `"$VAR/x`).
# Without that, ci.yml's own shellcheck argument list —
#
#     $ shellcheck --severity=warning \
#         scripts/generate-bindings.sh \
#
# — satisfied the assertion for ci.yml, which made it unconditionally true: the
# guard must lint the generator, so that line is permanent, and deleting the
# actual regeneration step still reported "ok". Linting a file is not running it.
DELEGATION='^[[:space:]]*(-[[:space:]]+)?(run:[[:space:]]+)?((exec[[:space:]]+)?(bash|sh)[[:space:]]+"?([^[:space:]"]*/)?|(exec[[:space:]]+)?"?[.$/][^[:space:]"]*/)generate-bindings\.sh([[:space:]]|"|$)'

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

# The npm-exposed build scripts, which must reach the generator through their
# build-uniffi-* counterpart. They used to be near-copies that built native
# libraries and regenerated nothing — the most travelled way to produce a
# mismatched artifact set, since six example READMEs start with
# `npm run build:all`. A script that generates nothing satisfies both checks
# above trivially, so it needs its own.
NATIVE_BUILD_ALIASES="
bindings/react-native/scripts/build-ios.sh:build-uniffi-ios.sh
bindings/react-native/scripts/build-android.sh:build-uniffi-android.sh
bindings/react-native/scripts/build-all.sh:build-uniffi-all.sh
"

# The negative-control fixtures, by name. Listed here with the other driver
# lists so they are size-pinned too: they are what proves the scan can fire.
HIT_FIXTURES="
inline
continued
global-option
cargo-run
gradle-kotlin
gradle-groovy
argv-list
nested-list
"

MISS_FIXTURES="
delegating
prose
prose-adjacent
"

# The three committed paths, repo-relative. CI deletes exactly these before
# regenerating and diffs exactly these afterwards; if the generator ever writes
# somewhere else, `git diff` on a path nothing wrote to reports no diff and the
# gate goes green on stale bindings. This list is the single record of what
# those three places must agree on, and it is checked against both.
EXPECTED_OUTPUTS="
swift:bindings/react-native/ios/Generated
kotlin:bindings/react-native/android/src/main/java
python:bindings/python/offline_protocol_sdk
"

# Total number of assertions a healthy run makes. This is the backstop for the
# whole "silently deleted assertion" class, which five review rounds hit five
# different ways: a trimmed driver list, a fixture whose creation was removed,
# a loop whose glob stopped matching. Each was fixed where it was found, and
# each time the next one appeared somewhere the previous fix did not reach.
# Counting is the check that does not need to anticipate the mechanism — a run
# that asserts fewer things than it used to is wrong however it got that way.
#
# It does mean this number moves when assertions are added or removed on
# purpose. That is the intended cost: the failure is loud, names the expected
# and actual counts, and updating it is a deliberate one-line acknowledgement
# that the guard's coverage changed.
EXPECTED_ASSERTIONS=54

FAILURES=0
ASSERTIONS=0

pass() {
  ASSERTIONS=$((ASSERTIONS + 1))
  echo "  ok — $*"
}

fail() {
  ASSERTIONS=$((ASSERTIONS + 1))
  echo "  FAIL — $*" >&2
  FAILURES=$((FAILURES + 1))
}

# Size pins for the lists that drive loops. Necessary but not sufficient on
# their own — a list can keep its length while losing coverage (swapping the
# python entry for a second swift entry left the count at 3 and the run at
# "all passed" with Python entirely untested), so the loops below also pin the
# *identity* of what they processed.
assert_list_size() {
  local name="$1" expected="$2" actual
  # `|| true` because `grep -c` exits 1 on a zero count, which under
  # `set -euo pipefail` would abort the script at this assignment — killing the
  # run before it could print the message written to explain exactly this case.
  actual="$(printf '%s\n' "$3" | grep -c . || true)"
  if [[ "$actual" -eq "$expected" ]]; then
    pass "$name still has $expected entries"
  else
    fail "$name has $actual entries, expected $expected — an emptied or trimmed list silently deletes the assertions it drives"
  fi
}

# Identity pin: the set a loop actually processed must be the set it was meant
# to. Catches duplicates and substitutions that leave the count intact.
# Sorted, not deduplicated: a duplicate must read as different from the set it
# displaced, which is the whole point (swapping python for a second swift kept
# every count correct).
normalize_set() {
  # sed rather than `grep .`: grep exits 1 on no matches, which under
  # `set -euo pipefail` kills the run at the caller's assignment — no FAIL
  # line, no summary, and the assertion-count backstop never reached. Same
  # defect as the one `|| true` fixes above, one function over.
  printf '%s\n' "$1" | tr '[:space:]' '\n' | sed '/^$/d' | sort | tr '\n' ' '
}

# A duplicated entry keeps `assert_list_size` correct AND keeps the assertion
# total correct, so both backstops report ok while the displaced entry's
# coverage is simply gone — duplicating the release.yml entry and deleting both
# of its real invocations left the run green at 50. That is the caller whose
# Kotlin is downloaded over the committed Android bindings at publish time, and
# §4's scan cannot backstop it: a caller that stops regenerating invokes no
# bindgen and so produces no offender.
#
# This rather than a second assert_set_equals: comparing a list against itself
# is the R1 self-comparison shape and catches nothing. EXPECTED_OUTPUTS can use
# assert_set_equals only because its expected side is a hardcoded literal.
assert_no_duplicates() {
  local name="$1" dupes
  dupes="$(printf '%s\n' "$2" | sed '/^$/d' | sort | uniq -d | tr '\n' ' ')"
  if [[ -z "$dupes" ]]; then
    pass "$name has no duplicated entries"
  else
    fail "$name repeats [${dupes% }] — a duplicated entry keeps the count while deleting the coverage of the entry it displaced"
  fi
}

assert_set_equals() {
  local name="$1" expected actual
  expected="$(normalize_set "$2")"
  actual="$(normalize_set "$3")"
  if [[ "$expected" == "$actual" ]]; then
    pass "$name covered exactly [${expected% }]"
  else
    fail "$name covered [${actual% }] but should have covered [${expected% }] — a duplicated or substituted entry drops coverage while keeping the count"
  fi
}

# Body of a named step in ci.yml, used to check that the workflow's own path
# lists have not drifted from EXPECTED_OUTPUTS. Steps in that file are indented
# six spaces.
ci_step_body() {
  awk -v want="      - name: $1" '
    index($0, want) == 1 { f = 1; next }
    f && /^      - name: / { exit }
    f { print }
  ' "$CI_WORKFLOW"
}

echo "Guarding the shared binding generator..."

assert_list_size EXPECTED_OUTPUTS 3 "$EXPECTED_OUTPUTS"
assert_list_size DELEGATING_CALLERS 6 "$DELEGATING_CALLERS"
assert_list_size NATIVE_BUILD_ALIASES 3 "$NATIVE_BUILD_ALIASES"
# EXPECTED_OUTPUTS is covered by assert_set_equals against a hardcoded literal;
# these four have no such literal to compare against.
assert_no_duplicates DELEGATING_CALLERS "$DELEGATING_CALLERS"
assert_no_duplicates NATIVE_BUILD_ALIASES "$NATIVE_BUILD_ALIASES"

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
#    it actually asked for — languages, output paths, UDL, and --no-format.
# ---------------------------------------------------------------------------

STUB_ROOT="$(mktemp -d)"
trap 'rm -rf "$STUB_ROOT"' EXIT

# Logs argv NUL-separated, one file per call, so the assertions below can read
# the arguments structurally. String-slicing the joined command line instead
# would make a correct refactor (flag reordering, `--out-dir=X`, generating to
# a temp dir and moving) report "never generated <language>" — a false failure
# whose message points at the wrong thing entirely.
make_stub() {
  local bin_dir="$1" reported="$2"
  mkdir -p "$bin_dir"
  cat >"$bin_dir/uniffi-bindgen" <<STUB
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then
  echo "uniffi-bindgen $reported"
  exit 0
fi
mkdir -p "\$STUB_LOG_DIR"
printf '%s\0' "\$@" >"\$(mktemp "\$STUB_LOG_DIR/call-XXXXXX")"
exit 0
STUB
  chmod +x "$bin_dir/uniffi-bindgen"
}

# Value of `--flag value` or `--flag=value`, from a NUL-separated argv file.
arg_value() {
  local file="$1" flag="$2" arg take=""
  while IFS= read -r -d '' arg; do
    if [[ -n "$take" ]]; then
      printf '%s' "$arg"
      return 0
    fi
    case "$arg" in
      "$flag") take=1 ;;
      "$flag"=*) printf '%s' "${arg#*=}"; return 0 ;;
    esac
  done <"$file"
  return 1
}

has_arg() {
  local file="$1" flag="$2" arg
  while IFS= read -r -d '' arg; do
    [[ "$arg" == "$flag" || "$arg" == "$flag"=* ]] && return 0
  done <"$file"
  return 1
}

CRATE_PIN="$(sed -n 's/^uniffi = "\([0-9.]*\)"$/\1/p' crates/offline-protocol-uniffi/Cargo.toml | head -1)"
PIN_MAJOR="$(printf '%s' "$CRATE_PIN" | cut -d. -f1)"
PIN_MINOR="$(printf '%s' "$CRATE_PIN" | cut -d. -f2)"
if [[ ! "$PIN_MAJOR" =~ ^[0-9]+$ || ! "$PIN_MINOR" =~ ^[0-9]+$ ]]; then
  fail "could not read a major.minor uniffi pin from crates/offline-protocol-uniffi/Cargo.toml (got '$CRATE_PIN') — the checks below cannot be trusted"
  PIN_MAJOR=0
  PIN_MINOR=0
fi

# Pulled once, and asserted non-empty: if a step is renamed these go blank and
# every path check below would "pass" against an empty haystack.
RM_STEP="$(ci_step_body 'Remove generated bindings so regeneration must replace them')"
DRIFT_STEP="$(ci_step_body 'Fail on stale bindings')"
if [[ -n "$RM_STEP" ]]; then
  pass "$CI_WORKFLOW has the pre-regeneration delete step"
else
  fail "$CI_WORKFLOW has no 'Remove generated bindings...' step — the drift gate can no longer tell 'up to date' from 'never written', and the per-path checks below are vacuous"
fi
if [[ -n "$DRIFT_STEP" ]]; then
  pass "$CI_WORKFLOW has the drift gate step"
else
  fail "$CI_WORKFLOW has no 'Fail on stale bindings' step — nothing checks the committed bindings, and the per-path checks below are vacuous"
fi

# Every path those two steps name must be tracked. `rm -f` and `git diff --
# <pathspec>` both succeed silently on a path that does not exist, so a typo in
# a leaf filename disables the delete and the diff for that file while both
# steps still go green — the same "empty haystack reads as clean" shape, one
# level below the directory the per-language checks pin.
untracked_ci_paths=""
for ci_path in $(printf '%s\n%s\n' "$RM_STEP" "$DRIFT_STEP" | grep -oE 'bindings/[^ \\]+' | sed 's/[;\\]*$//' | sort -u); do
  git ls-files --error-unmatch "$ci_path" >/dev/null 2>&1 || untracked_ci_paths="$untracked_ci_paths $ci_path"
done
if [[ -z "$untracked_ci_paths" ]]; then
  pass "every path $CI_WORKFLOW deletes and diffs is tracked"
else
  fail "$CI_WORKFLOW names untracked path(s) —$untracked_ci_paths — rm and git diff both succeed silently on those, so they are neither deleted nor checked"
fi

ok_bin="$STUB_ROOT/ok"
call_dir="$STUB_ROOT/calls"
mkdir -p "$call_dir"
# A patch level the pin does not name, so a passing run also proves the version
# gate tolerates patch drift (uniffi's FFI contract is not patch-scoped) rather
# than only that the gate exists.
make_stub "$ok_bin" "$PIN_MAJOR.$PIN_MINOR.99"

if PATH="$ok_bin:$PATH" STUB_LOG_DIR="$call_dir" bash "$GENERATOR" >"$STUB_ROOT/out" 2>&1; then
  pass "$GENERATOR runs to completion (patch-level version drift tolerated)"
else
  fail "$GENERATOR failed against a stub bindgen: $(tail -3 "$STUB_ROOT/out" | tr '\n' ' ')"
fi

call_count="$(find "$call_dir" -type f | wc -l | tr -d ' ')"
if [[ "$call_count" -eq 3 ]]; then
  pass "it invoked bindgen exactly 3 times — one per language"
else
  fail "it invoked bindgen $call_count time(s), expected 3 (Swift, Kotlin, Python)"
fi

languages_seen=""
while IFS= read -r spec; do
  [[ -z "$spec" ]] && continue
  language="${spec%%:*}"
  expected_dir="${spec#*:}"
  languages_seen="$languages_seen $language"

  call=""
  for candidate in "$call_dir"/*; do
    [[ -f "$candidate" ]] || continue
    if [[ "$(arg_value "$candidate" --language || true)" == "$language" ]]; then
      call="$candidate"
      break
    fi
  done

  if [[ -z "$call" ]]; then
    fail "$GENERATOR never generated $language — a caller would ship a partial set"
    continue
  fi

  actual_dir="$(arg_value "$call" --out-dir || true)"
  actual_dir="${actual_dir#"$REPO_ROOT"/}"
  if [[ "$actual_dir" == "$expected_dir" ]]; then
    pass "$language -> $expected_dir"
  else
    fail "$language written to '$actual_dir', but CI deletes and diffs '$expected_dir' — a mismatch here is invisible (git diff on a path nothing wrote to reports no diff)"
    # The generator mkdir -p's its out-dir, so a wrong path may have just been
    # created. Take it back if empty, but never climb outside the repo.
    case "$actual_dir" in
      /* | *..*) : ;;
      "") : ;;
      *) rmdir -p "$actual_dir" 2>/dev/null || true ;;
    esac
  fi

  # The workflow's delete list and drift-gate list are two more copies of this
  # path. Gutting either one restores the silent-stale-bindings hole without
  # touching the generator, so pin them here.
  if printf '%s' "$RM_STEP" | grep -Fq "$expected_dir"; then
    pass "$CI_WORKFLOW deletes $language output before regenerating"
  else
    fail "$CI_WORKFLOW no longer deletes '$expected_dir' before regenerating — the drift gate can no longer tell 'up to date' from 'never written'"
  fi
  if printf '%s' "$DRIFT_STEP" | grep -Fq "$expected_dir"; then
    pass "$CI_WORKFLOW diffs $language output"
  else
    fail "$CI_WORKFLOW no longer diffs '$expected_dir' — stale $language bindings would ship unnoticed"
  fi

  if has_arg "$call" --no-format; then
    pass "$language is generated with --no-format"
  else
    fail "$language is generated without --no-format — output would depend on whether ktlint/swiftformat are installed"
  fi

  if grep -qa 'offline_protocol.udl' "$call"; then
    pass "$language is generated from the UDL"
  else
    fail "$language was not generated from src/offline_protocol.udl"
  fi
done <<EOF
$EXPECTED_OUTPUTS
EOF

assert_set_equals "the per-language checks" "swift kotlin python" "$languages_seen"

# ---------------------------------------------------------------------------
# 3. Behavioral: the version gate actually rejects a mismatched bindgen.
# ---------------------------------------------------------------------------

bad_bin="$STUB_ROOT/bad"
make_stub "$bad_bin" "$PIN_MAJOR.$((PIN_MINOR + 1)).0"

if [[ ! -x "$bad_bin/uniffi-bindgen" ]]; then
  # Without this the check is worse than absent: with no stub on PATH and no
  # real bindgen (the CI ordering — this runs before `cargo install uniffi`)
  # the generator exits 1 with "not found", which reads exactly like "mismatch
  # rejected" and reports ok.
  fail "the mismatched-bindgen stub was not created — this check would pass vacuously"
elif mkdir -p "$STUB_ROOT/bad-calls" \
  && PATH="$bad_bin:$PATH" STUB_LOG_DIR="$STUB_ROOT/bad-calls" bash "$GENERATOR" >/dev/null 2>&1; then
  fail "$GENERATOR generated with a bindgen whose minor version disagrees with the crate pin — those bindings' checksums fail at runtime, not at build time"
else
  # A non-zero exit alone does not mean nothing was generated, and this check
  # is about *when* the refusal happens. Move the comparison below the three
  # generate calls and the generator still exits non-zero — having already
  # written all three binding sets with the wrong bindgen, which is precisely
  # the outcome the pin exists to prevent. The stub logs every call it is
  # asked to make, so assert it was asked for none. (Zero is right: the
  # --version branch returns before logging.)
  bad_calls="$(find "$STUB_ROOT/bad-calls" -type f | wc -l | tr -d ' ')"
  if [[ "$bad_calls" -eq 0 ]]; then
    pass "a bindgen disagreeing with the crate pin is rejected before anything is written"
  else
    fail "$GENERATOR refused the mismatched bindgen only after invoking it $bad_calls time(s) — the bindings were already written with the wrong bindgen"
  fi
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
# 5. The enumerated callers still delegate, and the npm build scripts still
#    route through a counterpart that does.
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

while IFS= read -r spec; do
  [[ -z "$spec" ]] && continue
  alias_script="${spec%%:*}"
  counterpart="${spec#*:}"
  if [[ ! -f "$alias_script" ]]; then
    fail "$alias_script is missing — package.json still exposes it as an npm build script"
  # Mirrors DELEGATION's alternation: an interpreter, or an explicitly rooted
  # path. Without it any line-initial token containing the counterpart's path
  # counted as routing — `COUNTERPART=scripts/build-uniffi-ios.sh` in a script
  # that then builds natively and generates nothing passed green. That is the
  # R4 shellcheck-argument bug verbatim, left standing one regex over.
  elif grep -Eq "^[[:space:]]*((exec[[:space:]]+)?(bash|sh)[[:space:]]+\"?([^[:space:]\"]*/)?|(exec[[:space:]]+)?\"?[.\$/][^[:space:]\"]*/)$counterpart([[:space:]]|\"|\$)" "$alias_script"; then
    pass "$(basename "$alias_script") routes through $counterpart"
  else
    fail "$alias_script no longer routes through $counterpart — it would build native artifacts without regenerating bindings"
  fi
done <<EOF
$NATIVE_BUILD_ALIASES
EOF

# ---------------------------------------------------------------------------
# 6. Negative controls for the scan regex. A detector that cannot fire is
#    indistinguishable from one that passes vacuously, so prove it catches
#    every invocation shape and ignores the benign ones.
# ---------------------------------------------------------------------------

fixtures="$STUB_ROOT/fixtures"
mkdir -p "$fixtures"

printf 'uniffi-bindgen generate x.udl --language swift\n'            >"$fixtures/hit-inline"
printf 'uniffi-bindgen \\\n  generate x.udl --language kotlin\n'     >"$fixtures/hit-continued"
printf 'uniffi-bindgen --config u.toml generate x.udl\n'             >"$fixtures/hit-global-option"
printf 'cargo run --bin uniffi-bindgen -- generate x.udl\n'          >"$fixtures/hit-cargo-run"
printf 'commandLine("uniffi-bindgen", "generate", "x.udl")\n'        >"$fixtures/hit-gradle-kotlin"
printf "commandLine 'uniffi-bindgen', 'generate', 'x.udl'\n"         >"$fixtures/hit-gradle-groovy"
printf 'subprocess.run(["uniffi-bindgen", "generate", "x.udl"])\n'   >"$fixtures/hit-argv-list"
printf "spawnSync('uniffi-bindgen', ['generate', 'x.udl'])\n"        >"$fixtures/hit-nested-list"
printf 'if command -v uniffi-bindgen; then bash %s; fi\n' "$GENERATOR" >"$fixtures/miss-delegating"
printf 'Run uniffi-bindgen to generate the Swift bindings.\n'        >"$fixtures/miss-prose"
printf 'the uniffi-bindgen generator produces code\n'                >"$fixtures/miss-prose-adjacent"

# Driven by an explicit list rather than a glob. A glob makes the fixture set
# its own silent driver list: deleting three of the eight hit- fixtures left
# the run green at 44 assertions, because the loop simply had less to do and
# the `-f` guard only fires when *nothing* matches. These negative controls are
# the only evidence INVOCATION can fire at all, so quietly shrinking them is
# the purest form of the vacuous pass.
assert_list_size HIT_FIXTURES 8 "$HIT_FIXTURES"
assert_list_size MISS_FIXTURES 3 "$MISS_FIXTURES"
assert_no_duplicates HIT_FIXTURES "$HIT_FIXTURES"
assert_no_duplicates MISS_FIXTURES "$MISS_FIXTURES"

while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  fixture="$fixtures/hit-$name"
  if [[ ! -f "$fixture" ]]; then
    fail "the hit fixture '$name' was never created — its negative control is vacuous"
  elif grep -Eq "$INVOCATION" "$fixture"; then
    pass "the scan detects $name invocations"
  else
    fail "the scan MISSES $name invocations — it would pass vacuously"
  fi
done <<EOF
$HIT_FIXTURES
EOF

while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  fixture="$fixtures/miss-$name"
  if [[ ! -f "$fixture" ]]; then
    fail "the miss fixture '$name' was never created — its negative control is vacuous"
  elif grep -Eq "$INVOCATION" "$fixture"; then
    fail "the scan falsely flags $name"
  else
    pass "the scan ignores $name"
  fi
done <<EOF
$MISS_FIXTURES
EOF

# Deliberately not routed through fail(), which would itself increment.
if [[ "$ASSERTIONS" -ne "$EXPECTED_ASSERTIONS" ]]; then
  echo "  FAIL — ran $ASSERTIONS assertions, expected $EXPECTED_ASSERTIONS — checks were added or silently deleted; if the change was intentional, update EXPECTED_ASSERTIONS" >&2
  FAILURES=$((FAILURES + 1))
fi

echo ""
if [[ "$FAILURES" -eq 0 ]]; then
  echo "All $ASSERTIONS binding-generator guards passed."
else
  echo "$FAILURES guard(s) failed." >&2
  exit 1
fi
