#!/usr/bin/env bash

# Drives scripts/maven-central-bundle.sh and scripts/maven-central-upload.sh.
#
# The bundle is signed with a throwaway key and checked the way Central checks
# it. The upload talks to a stand-in curl that plays the Portal's answers, so
# every branch runs without a network or a credential: a publish, a rehearsal
# (validated, then dropped), a refused deployment, and the re-run of a release
# that is already on Maven Central (nothing uploaded). Needs gpg, jq, zip and
# coreutils: a Linux runner.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUNDLER="$REPO_ROOT/scripts/maven-central-bundle.sh"
UPLOADER="$REPO_ROOT/scripts/maven-central-upload.sh"

FAILURES=0
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass() { echo "  ok - $*"; }
fail() {
  echo "  FAIL - $*" >&2
  FAILURES=$((FAILURES + 1))
}

VERSION="1.2.3-rc.1"
COORDINATE="com/offlineprotocol/offline-protocol-android"
ARTIFACT="offline-protocol-android-$VERSION"

# A Maven repository the way Gradle writes one: checksums of every kind and a
# maven-metadata.xml.
build_repository() {
  local root="$1" dir="$1/$COORDINATE/$VERSION" file
  mkdir -p "$dir"
  for file in "$ARTIFACT.aar" "$ARTIFACT.pom" "$ARTIFACT.module" "$ARTIFACT-sources.jar" "$ARTIFACT-javadoc.jar"; do
    echo "fixture $file" >"$dir/$file"
    echo "stale" >"$dir/$file.md5"
    echo "stale" >"$dir/$file.sha512"
  done
  echo "<metadata/>" >"$root/$COORDINATE/maven-metadata.xml"
}

# A throwaway signing key, exported the way the secret holds it.
export GNUPGHOME="$WORK/keyring"
mkdir -m 700 "$GNUPGHOME"
gpg --batch --quiet --pinentry-mode loopback --passphrase fixture-passphrase \
  --quick-gen-key "Fixture Signer <fixture@example.com>" ed25519 sign never
SIGNING_KEY="$(gpg --batch --quiet --pinentry-mode loopback --passphrase fixture-passphrase \
  --armor --export-secret-keys)"
gpg --batch --quiet --armor --export >"$WORK/public.asc"
unset GNUPGHOME

bundle() {
  SIGNING_KEY="$SIGNING_KEY" SIGNING_KEY_PASSWORD="${2-fixture-passphrase}" \
    bash "$BUNDLER" "$1" "$WORK/bundle.zip" "$VERSION"
}

echo "== bundle =="
build_repository "$WORK/repo"
if bundle "$WORK/repo" >"$WORK/bundle.log" 2>&1; then
  pass "bundled"
else
  fail "bundling a complete repository failed"
  cat "$WORK/bundle.log" >&2
fi
EXTRACT="$WORK/extract"
mkdir -p "$EXTRACT"
unzip -qq "$WORK/bundle.zip" -d "$EXTRACT"
export GNUPGHOME="$WORK/verify"
mkdir -m 700 "$GNUPGHOME"
gpg --batch --quiet --import "$WORK/public.asc"
for file in "$ARTIFACT.aar" "$ARTIFACT.pom" "$ARTIFACT.module" "$ARTIFACT-sources.jar" "$ARTIFACT-javadoc.jar"; do
  path="$EXTRACT/$COORDINATE/$VERSION/$file"
  if gpg --batch --quiet --verify "$path.asc" "$path" 2>/dev/null; then
    pass "$file is signed by the key"
  else
    fail "$file has no valid signature"
  fi
  if [ "$(cat "$path.md5")" = "$(md5sum "$path" | cut -d' ' -f1)" ] &&
    [ "$(cat "$path.sha1")" = "$(sha1sum "$path" | cut -d' ' -f1)" ]; then
    pass "$file carries its own MD5 and SHA-1"
  else
    fail "$file's checksums are not its own"
  fi
done
unset GNUPGHOME
if [ -z "$(find "$EXTRACT" -name 'maven-metadata.xml*' -o -name '*.sha512')" ]; then
  pass "no maven-metadata.xml and no build checksums in the bundle"
else
  fail "the bundle carries files Central does not want: $(find "$EXTRACT" -name 'maven-metadata.xml*' -o -name '*.sha512')"
fi

echo "== bundle refusals =="
expect_bundle_failure() {
  local label="$1" mutate="$2" root="$WORK/neg-$1"
  build_repository "$root"
  "$mutate" "$root"
  if bundle "$root" "${3-fixture-passphrase}" >"$WORK/neg-$label.log" 2>&1; then
    fail "$label: bundled, and should have refused"
  else
    pass "$label: refused"
  fi
}
no_change() { :; }
drop_javadoc() { rm -f "$1/$COORDINATE/$VERSION/$ARTIFACT-javadoc.jar"; }
extra_file() { echo extra >"$1/$COORDINATE/$VERSION/$ARTIFACT-extra.jar"; }
second_version() { mkdir -p "$1/$COORDINATE/0.0.1"; }
expect_bundle_failure "missing-javadoc" drop_javadoc
expect_bundle_failure "unexpected-file" extra_file
expect_bundle_failure "two-versions" second_version
expect_bundle_failure "wrong-passphrase" no_change wrong-passphrase

# ---------------------------------------------------------------------------
# A stand-in curl that plays the Portal. It answers the published-POM probe
# from FAKE_ON_CENTRAL, the upload with a deployment id, each status request
# with the next state in FAKE_STATES, and logs every request.
# ---------------------------------------------------------------------------

FAKE_BIN="$WORK/bin"
mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/curl" <<'FAKE'
#!/usr/bin/env bash
url="" method=GET head=0
for argument in "$@"; do
  case "$argument" in
    http*) url="$argument" ;;
    -sSfI | -I) head=1 ;;
    POST | DELETE) method="$argument" ;;
  esac
done
echo "$method $url" >>"$FAKE_LOG"
if [ "$head" = 1 ]; then
  [ "${FAKE_ON_CENTRAL:-0}" = 1 ] && exit 0
  exit 22
fi
case "$url" in
  */upload\?*) echo "dep-0001" ;;
  */status\?*)
    count="$(grep -c '/status?' "$FAKE_LOG")"
    read -r -a states <<<"$FAKE_STATES"
    state="${states[$((count - 1))]:-${states[-1]}}"
    if [ "$state" = FAILED ]; then
      printf '{"deploymentState":"FAILED","errors":{"pkg:maven/com.offlineprotocol/offline-protocol-android":["Invalid signature"]}}'
    else
      printf '{"deploymentState":"%s"}' "$state"
    fi
    ;;
  */deployment/*) ;;
esac
FAKE
chmod +x "$FAKE_BIN/curl"

printf 'not a real bundle' >"$WORK/upload.zip"

upload() {
  local mode="$1" states="$2" on_central="${3:-0}"
  : >"$WORK/curl.log"
  PATH="$FAKE_BIN:$PATH" FAKE_LOG="$WORK/curl.log" FAKE_STATES="$states" FAKE_ON_CENTRAL="$on_central" \
    MAVEN_CENTRAL_USERNAME=user MAVEN_CENTRAL_PASSWORD=secret POLL_SECONDS=0 POLLS=5 \
    CENTRAL_API=https://portal.example/api CENTRAL_REPOSITORY=https://repo.example \
    bash "$UPLOADER" "$WORK/upload.zip" "$mode" "$VERSION"
}

echo "== upload =="
if upload publish "PENDING VALIDATING PUBLISHING" >"$WORK/publish.log" 2>&1; then
  pass "a publish that reaches PUBLISHING succeeds"
else
  fail "a publish that reaches PUBLISHING failed"
  cat "$WORK/publish.log" >&2
fi
if grep -q 'publishingType=AUTOMATIC' "$WORK/curl.log" && ! grep -q '^DELETE' "$WORK/curl.log"; then
  pass "it uploads as AUTOMATIC and drops nothing"
else
  fail "a publish did not upload as AUTOMATIC, or dropped the deployment"
  cat "$WORK/curl.log" >&2
fi

if upload rehearse "VALIDATING VALIDATED" >"$WORK/rehearse.log" 2>&1; then
  pass "a rehearsal that validates succeeds"
else
  fail "a rehearsal that validates failed"
  cat "$WORK/rehearse.log" >&2
fi
if grep -q 'publishingType=USER_MANAGED' "$WORK/curl.log" &&
  grep -q '^DELETE .*/deployment/dep-0001' "$WORK/curl.log"; then
  pass "it uploads as USER_MANAGED and drops the deployment"
else
  fail "a rehearsal did not upload as USER_MANAGED and drop the deployment"
  cat "$WORK/curl.log" >&2
fi

if upload rehearse "VALIDATING PUBLISHING" >"$WORK/rehearse-published.log" 2>&1; then
  fail "a rehearsal whose deployment publishes was accepted"
else
  pass "a rehearsal whose deployment publishes fails"
fi

if upload publish "VALIDATING FAILED" >"$WORK/failed.log" 2>&1; then
  fail "a deployment that failed validation was accepted"
else
  if grep -q 'Invalid signature' "$WORK/failed.log"; then
    pass "a failed deployment fails, and prints the Portal's reasons"
  else
    fail "a failed deployment did not print the Portal's reasons"
    cat "$WORK/failed.log" >&2
  fi
fi

if upload publish "VALIDATING" >"$WORK/stuck.log" 2>&1; then
  fail "a deployment that never left VALIDATING was accepted"
else
  pass "a deployment that never leaves VALIDATING times out"
fi

if upload publish "PUBLISHED" 1 >"$WORK/rerun.log" 2>&1 && ! grep -q 'upload' "$WORK/curl.log"; then
  pass "a version already on Maven Central is not uploaded again"
else
  fail "a version already on Maven Central was uploaded again"
  cat "$WORK/curl.log" >&2
fi

if upload rehearse "VALIDATED" 1 >/dev/null 2>&1 && grep -q 'upload' "$WORK/curl.log"; then
  pass "a rehearsal uploads even when the version is on Maven Central"
else
  fail "a rehearsal skipped its upload"
fi

if ! grep -rq 'secret' "$WORK"/*.log; then
  pass "the token is never printed"
else
  fail "a log carries the token"
fi

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "All Maven Central checks passed."
else
  echo "$FAILURES Maven Central check(s) failed." >&2
  exit 1
fi
