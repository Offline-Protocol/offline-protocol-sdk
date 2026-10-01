#!/usr/bin/env bash

# Drives scripts/publish-swift-package.sh against local bare repositories
# standing in for the distribution repository.
#
# The distribution repository's workflow runs it once per version, and what
# it gets wrong cannot be taken back: a tag that moves breaks every consumer
# that resolved it. So this pins the first publish, the re-run of the same
# release (accepted, nothing pushed), the same version with different content
# (refused, tag untouched), a later release on top, a rehearsal (nothing
# pushed), the workflow directory surviving the tree replacement, and the
# refusals of a package whose manifest does not match the archive beside it.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PUBLISH="$REPO_ROOT/scripts/publish-swift-package.sh"
ASSEMBLE="$REPO_ROOT/scripts/assemble-swift-package.sh"
ARCHIVE_URL="$REPO_ROOT/scripts/swiftpm-archive-url.sh"

FAILURES=0
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass() { echo "  ok - $*"; }
fail() {
  echo "  FAIL - $*" >&2
  FAILURES=$((FAILURES + 1))
}

sha() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

printf 'first archive' >"$WORK/first.zip"
printf 'second archive' >"$WORK/second.zip"

# The package a release attaches: assembled from this checkout against the
# archive's url and checksum, as the release job assembles it.
package() {
  local version="$1" archive="$2" stem out dir
  stem="$version-$(basename "$archive" .zip)"
  out="$WORK/package-$stem.tar.gz"
  dir="$WORK/assembled-$stem"
  bash "$ASSEMBLE" --output "$dir" --version "$version" \
    --xcframework-url "$(bash "$ARCHIVE_URL" "$version")" \
    --xcframework-checksum "$(sha "$archive")" >/dev/null
  tar -czf "$out" -C "$dir" .
  printf '%s' "$out"
}

# As the distribution repository's workflow calls it: with GITHUB_REPOSITORY
# naming THAT repository, which the script must not read. The repository the
# release is on is named outright.
publish() {
  local remote="$1" version="$2" package="$3" archive="$4" mode="$5"
  GITHUB_SHA=deadbeef GITHUB_REPOSITORY=Offline-Protocol/offline-protocol-swift \
    bash "$PUBLISH" --version "$version" --repository Offline-Protocol/offline-protocol-sdk \
    --package "$package" --archive "$archive" --remote "$remote" --mode "$mode"
}

remote_ref() { git -C "$1" rev-parse --verify --quiet "$2" || true; }

# The distribution repository as it is before any release: the workflow that
# runs this, on main.
seed_remote() {
  local remote="$1" seed
  seed="$WORK/seed-$(basename "$remote" .git)"
  # Named: a bare repository's HEAD points at git's configured default
  # branch, which is `master` where nothing set it, and a clone of it then
  # checks out nothing while main holds the workflow.
  git init --quiet --bare --initial-branch=main "$remote"
  git init --quiet -b main "$seed"
  mkdir -p "$seed/.github/workflows"
  echo "name: Publish" >"$seed/.github/workflows/publish.yml"
  git -C "$seed" add -A
  git -C "$seed" -c user.name=seed -c user.email=seed@example.com commit --quiet -m "The publishing workflow"
  git -C "$seed" push --quiet "$remote" main
}

FIRST="$(package 1.0.0 "$WORK/first.zip")"

echo "== first publish into a repository that holds the workflow =="
REMOTE="$WORK/dist.git"
seed_remote "$REMOTE"
if publish "$REMOTE" 1.0.0 "$FIRST" "$WORK/first.zip" push >"$WORK/first.log" 2>&1; then
  pass "published 1.0.0"
else
  fail "the first publish failed"
  cat "$WORK/first.log" >&2
fi
TAG_COMMIT="$(remote_ref "$REMOTE" 'refs/tags/1.0.0^{commit}')"
MAIN="$(remote_ref "$REMOTE" refs/heads/main)"
if [ -n "$TAG_COMMIT" ] && [ "$TAG_COMMIT" = "$MAIN" ]; then
  pass "the tag and main name the same commit"
else
  fail "tag 1.0.0 ($TAG_COMMIT) and main ($MAIN) disagree"
fi
MANIFEST="$(git -C "$REMOTE" show 1.0.0:Package.swift 2>/dev/null || true)"
if grep -qF "$(bash "$ARCHIVE_URL" 1.0.0)" <<<"$MANIFEST" && grep -qF "$(sha "$WORK/first.zip")" <<<"$MANIFEST"; then
  pass "the tagged manifest names the archive and its checksum"
else
  fail "the tagged manifest does not name the archive and its checksum"
fi
if [ "$(git -C "$REMOTE" cat-file -t refs/tags/1.0.0)" = tag ]; then
  pass "the tag is annotated"
else
  fail "the tag is lightweight"
fi
if git -C "$REMOTE" cat-file -e "1.0.0:.github/workflows/publish.yml" 2>/dev/null; then
  pass "the workflow survived the tree replacement"
else
  fail "the workflow was deleted with the rest of the tree"
fi
if git -C "$REMOTE" cat-file -e "1.0.0:Sources/OfflineProtocolSDK/Generated/offline_protocol.swift" 2>/dev/null; then
  pass "the package is in the tagged tree"
else
  fail "the tagged tree has no package"
fi

echo "== the same release again =="
if publish "$REMOTE" 1.0.0 "$FIRST" "$WORK/first.zip" push >"$WORK/rerun.log" 2>&1 &&
  grep -q 'already published' "$WORK/rerun.log"; then
  pass "a re-run of the same release is accepted"
else
  fail "a re-run of the same release was not accepted as already published"
  cat "$WORK/rerun.log" >&2
fi
if [ "$(remote_ref "$REMOTE" refs/heads/main)" = "$MAIN" ]; then
  pass "the re-run pushed nothing"
else
  fail "the re-run moved main"
fi

echo "== the same release again, after the workflow changed =="
EDIT="$WORK/edit"
git clone --quiet "$REMOTE" "$EDIT"
echo "name: Publish (edited)" >"$EDIT/.github/workflows/publish.yml"
git -C "$EDIT" -c user.name=seed -c user.email=seed@example.com commit --quiet -am "Edit the workflow"
git -C "$EDIT" push --quiet origin main
if publish "$REMOTE" 1.0.0 "$FIRST" "$WORK/first.zip" push >"$WORK/rerun2.log" 2>&1 &&
  grep -q 'already published' "$WORK/rerun2.log"; then
  pass "a changed workflow does not make the re-run look like a moved tag"
else
  fail "a changed workflow was mistaken for different package content"
  cat "$WORK/rerun2.log" >&2
fi

echo "== the same version with different content =="
FIRST_SECOND="$(package 1.0.0 "$WORK/second.zip")"
if publish "$REMOTE" 1.0.0 "$FIRST_SECOND" "$WORK/second.zip" push >"$WORK/moved.log" 2>&1; then
  fail "a second archive under a published version was accepted"
else
  if grep -q 'never moves' "$WORK/moved.log"; then
    pass "refused, and says why"
  else
    fail "refused for another reason"
    cat "$WORK/moved.log" >&2
  fi
fi
if [ "$(remote_ref "$REMOTE" 'refs/tags/1.0.0^{commit}')" = "$TAG_COMMIT" ]; then
  pass "the tag did not move"
else
  fail "the tag moved"
fi

echo "== a later release =="
LATER_PKG="$(package 1.1.0-rc.1 "$WORK/second.zip")"
if publish "$REMOTE" 1.1.0-rc.1 "$LATER_PKG" "$WORK/second.zip" push >"$WORK/later.log" 2>&1; then
  pass "published 1.1.0-rc.1"
else
  fail "the later release failed"
  cat "$WORK/later.log" >&2
fi
LATER="$(remote_ref "$REMOTE" 'refs/tags/1.1.0-rc.1^{commit}')"
if [ -n "$LATER" ] && git -C "$REMOTE" merge-base --is-ancestor "$TAG_COMMIT" "$LATER"; then
  pass "it sits on top of the previous release"
else
  fail "it does not sit on top of the previous release"
fi

echo "== a rehearsal, into an empty repository =="
FRESH="$WORK/fresh.git"
git init --quiet --bare "$FRESH"
TWO="$(package 2.0.0 "$WORK/first.zip")"
if publish "$FRESH" 2.0.0 "$TWO" "$WORK/first.zip" rehearse >"$WORK/rehearse.log" 2>&1; then
  pass "the rehearsal succeeded"
else
  fail "the rehearsal failed"
  cat "$WORK/rehearse.log" >&2
fi
if [ -z "$(git -C "$FRESH" for-each-ref)" ]; then
  pass "the rehearsal pushed nothing"
else
  fail "the rehearsal pushed: $(git -C "$FRESH" for-each-ref)"
fi

echo "== refusals =="
# Each names the reason it expects, so a refusal for some other reason (an
# argument that does not exist, say) is not a pass.
expect_refusal() {
  local label="$1" reason="$2"
  shift 2
  if publish "$@" >"$WORK/refuse-$label.log" 2>&1; then
    fail "$label: accepted"
  elif grep -q "$reason" "$WORK/refuse-$label.log"; then
    pass "$label: refused"
  else
    fail "$label: refused for another reason"
    cat "$WORK/refuse-$label.log" >&2
  fi
}
# The archive beside the package is not the one the manifest names.
expect_refusal "archive-checksum-mismatch" "could not resolve" "$FRESH" 2.0.0 "$TWO" "$WORK/second.zip" push
# The manifest names another release's url.
expect_refusal "wrong-version-for-the-package" "publishes the archive at" "$FRESH" 2.0.1 "$TWO" "$WORK/first.zip" push
# Not a package.
printf 'not a package' >"$WORK/plain.txt"
tar -czf "$WORK/plain.tar.gz" -C "$WORK" plain.txt
expect_refusal "not-a-package" "no Package.swift" "$FRESH" 2.0.0 "$WORK/plain.tar.gz" "$WORK/first.zip" push
# A package that would overwrite the workflow.
GH_PKG_DIR="$WORK/with-github"
mkdir -p "$GH_PKG_DIR" && tar -xzf "$TWO" -C "$GH_PKG_DIR"
mkdir -p "$GH_PKG_DIR/.github/workflows" && echo "x" >"$GH_PKG_DIR/.github/workflows/publish.yml"
tar -czf "$WORK/with-github.tar.gz" -C "$GH_PKG_DIR" .
expect_refusal "package-carries-a-workflow" "carries a .github" "$FRESH" 2.0.0 "$WORK/with-github.tar.gz" "$WORK/first.zip" push
expect_refusal "no-such-remote" "cannot clone" "$WORK/no-such.git" 2.0.0 "$TWO" "$WORK/first.zip" push
expect_refusal "unknown-mode" "must be push or rehearse" "$FRESH" 2.0.0 "$TWO" "$WORK/first.zip" publish
if GITHUB_REPOSITORY=Offline-Protocol/offline-protocol-swift bash "$PUBLISH" --version 2.0.0 \
  --package "$TWO" --archive "$WORK/first.zip" --remote "$FRESH" --mode push >"$WORK/refuse-no-repository.log" 2>&1; then
  fail "no-repository: accepted without --repository"
elif grep -q -- '--repository' "$WORK/refuse-no-repository.log"; then
  pass "no-repository: refused"
else
  fail "no-repository: refused for another reason"
fi
# The token in a remote url is not printed when the clone fails.
if bash "$PUBLISH" --version 2.0.0 --repository Offline-Protocol/offline-protocol-sdk --package "$TWO" \
  --archive "$WORK/first.zip" --remote "https://x-access-token:hunter2secret@localhost:9/nope.git" --mode push \
  >"$WORK/refuse-token.log" 2>&1; then
  fail "token-remote: accepted"
elif grep -q hunter2secret "$WORK/refuse-token.log"; then
  fail "token-remote: the token was printed"
else
  pass "token-remote: refused without printing the token"
fi
if [ -z "$(git -C "$FRESH" for-each-ref)" ]; then
  pass "no refusal pushed anything"
else
  fail "a refusal pushed: $(git -C "$FRESH" for-each-ref)"
fi

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "All Swift publishing checks passed."
else
  echo "$FAILURES Swift publishing check(s) failed." >&2
  exit 1
fi
