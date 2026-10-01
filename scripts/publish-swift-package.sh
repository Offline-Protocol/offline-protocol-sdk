#!/usr/bin/env bash

# Publish the Swift package: commit the assembled package to the distribution
# repository and tag it with the release version (ADR 0025, decision 7).
#
# Run by the distribution repository's own workflow, after the release of the
# SDK is complete, on the package and the archive that release attached to
# its GitHub release, both verified against the release's attestations before
# this is called. Nothing pushes into the distribution repository from
# outside: it pulls, with the token every workflow holds for its own
# repository, so no credential for it exists anywhere.
#
# Swift Package Manager resolves a version to the revision its tag names and
# records that revision in every consumer's Package.resolved. A tag that moves
# afterwards fails every one of those builds, so a tag is never moved: a
# version already in the repository is accepted only when it holds exactly
# what this run would publish (a re-run of the same release), and refused
# otherwise. The archive the manifest names is checked against the bytes the
# release published, because a consumer resolves the tag and then downloads
# the archive, and a checksum that does not match is a package nobody can
# resolve.
#
# The distribution repository is generated output, except for .github/, which
# holds the workflow that runs this. Every release replaces the rest of its
# tree, so a file deleted from the package is deleted there too, and nobody
# edits it by hand.
#
# Usage:
#   bash scripts/publish-swift-package.sh \
#     --version <x.y.z> --repository <owner/repo> \
#     --package <offline-protocol-x.y.z-swift-package.tar.gz> \
#     --archive <offline-protocol-x.y.z-swiftpm-xcframework.zip> \
#     --remote <git remote> --mode <push|rehearse>
#
#   --repository      the repository whose release holds the archive: the
#                     SDK's, named outright. The url helper otherwise reads
#                     GITHUB_REPOSITORY, which in the distribution
#                     repository's own workflow names that repository, and
#                     every manifest would be refused.
#   --mode rehearse   everything, then `git push --dry-run`, which still asks
#                     the remote to accept the push.
#
# Environment:
#   GITHUB_SHA  optional, recorded in the commit message

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() {
  echo "ERROR: $*" >&2
  exit 1
}

VERSION=""
REPOSITORY=""
PACKAGE=""
ARCHIVE=""
REMOTE=""
MODE=""

while [ "$#" -gt 0 ]; do
  [ "$#" -ge 2 ] || die "$1 takes a value"
  case "$1" in
    --version) VERSION="$2" ;;
    --repository) REPOSITORY="$2" ;;
    --package) PACKAGE="$2" ;;
    --archive) ARCHIVE="$2" ;;
    --remote) REMOTE="$2" ;;
    --mode) MODE="$2" ;;
    *) die "unknown argument: $1" ;;
  esac
  shift 2
done

[ -n "$VERSION" ] && [ -n "$REPOSITORY" ] && [ -n "$PACKAGE" ] && [ -n "$ARCHIVE" ] && [ -n "$REMOTE" ] ||
  die "--version, --repository, --package, --archive and --remote are required"
[[ "$REPOSITORY" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || die "--repository must be owner/repo, not '$REPOSITORY'"
case "$MODE" in
  push | rehearse) ;;
  *) die "--mode must be push or rehearse, not '$MODE'" ;;
esac
[ -f "$PACKAGE" ] || die "no such package: $PACKAGE"
[ -f "$ARCHIVE" ] || die "no such archive: $ARCHIVE"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir "$WORK/package"
tar -xzf "$PACKAGE" -C "$WORK/package"
MANIFEST="$WORK/package/Package.swift"
[ -f "$MANIFEST" ] || die "the package has no Package.swift at its top level"
[ -d "$WORK/package/Sources" ] || die "the package has no Sources directory"
[ ! -e "$WORK/package/.github" ] || die "the package carries a .github directory, which belongs to the distribution repository"

# What the manifest names, read back with the shape the assemble script
# writes, and held to the archive beside it and to the url the release
# publishes under.
NAMED_URL="$(sed -n 's/^[[:space:]]*url: "\([^"]*\)",*$/\1/p' "$MANIFEST")"
NAMED_CHECKSUM="$(sed -n 's/^[[:space:]]*checksum: "\([^"]*\)"$/\1/p' "$MANIFEST")"
[ "$(printf '%s\n' "$NAMED_URL" | grep -c .)" = 1 ] || die "the manifest does not name exactly one archive url"
[ "$(printf '%s\n' "$NAMED_CHECKSUM" | grep -c .)" = 1 ] || die "the manifest does not name exactly one checksum"
EXPECTED_URL="$(GITHUB_REPOSITORY="$REPOSITORY" bash "$SCRIPT_DIR/swiftpm-archive-url.sh" "$VERSION")"
[ "$NAMED_URL" = "$EXPECTED_URL" ] ||
  die "the manifest names $NAMED_URL, and release $VERSION publishes the archive at $EXPECTED_URL"
CHECKSUM="$(sha256 "$ARCHIVE")"
[ "$NAMED_CHECKSUM" = "$CHECKSUM" ] ||
  die "the manifest names checksum $NAMED_CHECKSUM, and the archive's is $CHECKSUM: a consumer could not resolve this package"

# Without the userinfo: run by hand, the remote may carry a token.
git clone --quiet "$REMOTE" "$WORK/repo" 2>"$WORK/clone.log" || {
  cat "$WORK/clone.log" >&2
  die "cannot clone ${REMOTE/\/\/*@/\/\/}"
}
cd "$WORK/repo"

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

# An empty repository has no main yet: the first release starts it.
if git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null; then
  git checkout --quiet -B main refs/remotes/origin/main
else
  git checkout --quiet --orphan main
fi

# The whole tree is replaced, except the workflow that is running this.
find . -mindepth 1 -maxdepth 1 ! -name .git ! -name .github -exec rm -rf {} +
cp -R "$WORK/package/." .
git add -A

if git ls-remote --exit-code --tags origin "refs/tags/$VERSION" >/dev/null 2>&1; then
  git fetch --quiet origin "refs/tags/$VERSION:refs/tags/$VERSION"
  # The package, and not the workflow beside it, is what the tag promises.
  if git diff --cached --quiet "refs/tags/$VERSION" -- . ':(exclude).github'; then
    echo "::notice title=Swift package already published::$VERSION is in the distribution repository with exactly this content. Nothing to push."
    exit 0
  fi
  die "$VERSION is already tagged in the distribution repository with different content. A Swift package tag never moves: every consumer that resolved it records its revision. Publish a new version instead."
fi

git commit --quiet --allow-empty -m "Release $VERSION" \
  -m "Assembled from Offline-Protocol/offline-protocol-sdk@${GITHUB_SHA:-unknown}. Generated output: do not edit."
git tag -a "$VERSION" -m "$VERSION"

PUSH=(git push --atomic origin HEAD:refs/heads/main "refs/tags/$VERSION")
if [ "$MODE" = rehearse ]; then
  "${PUSH[@]}" --dry-run
  echo "Rehearsed: the distribution repository would accept $VERSION (archive checksum $CHECKSUM)."
else
  "${PUSH[@]}"
  echo "Published the Swift package $VERSION (archive checksum $CHECKSUM)."
fi
