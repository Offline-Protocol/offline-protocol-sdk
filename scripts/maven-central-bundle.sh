#!/usr/bin/env bash

# Sign the Android library's Maven repository and pack it as a Central Portal
# bundle.
#
# The library is built, tested and written out as a Maven repository by a job
# that holds no credentials. Signing happens here instead, in the publishing
# job, with gpg and nothing else: the signing key never enters a Gradle build,
# where every plugin and every dependency of the build runs beside it.
#
# Maven Central refuses a deployment unless every file carries a detached
# signature and an MD5 and SHA-1 checksum. The checksums are written here,
# not taken from the build, so what is uploaded is what was hashed.
# maven-metadata.xml is left out: Central writes its own.
#
# Usage:
#   SIGNING_KEY=<armored private key> [SIGNING_KEY_PASSWORD=<passphrase>] \
#     bash scripts/maven-central-bundle.sh <maven repository> <bundle.zip> <version>
#
# Prints the signing key's fingerprint, so the log says which key signed.

set -euo pipefail

die() {
  echo "ERROR: $*" >&2
  exit 1
}

[ "$#" -eq 3 ] || die "usage: $0 <maven repository> <bundle.zip> <version>"
REPOSITORY="$1"
BUNDLE="$2"
VERSION="$3"
: "${SIGNING_KEY:?SIGNING_KEY must hold the armored private signing key}"

COORDINATE="com/offlineprotocol/offline-protocol-sdk"
ARTIFACT="offline-protocol-sdk-$VERSION"
SOURCE="$REPOSITORY/$COORDINATE/$VERSION"

[ -d "$SOURCE" ] || die "no $VERSION in the repository: $SOURCE"
VERSIONS="$(find "$REPOSITORY/$COORDINATE" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
[ "$VERSIONS" = 1 ] || die "the repository holds $VERSIONS versions; a bundle is one release"

# What Central requires of an Android library: the artifact, its POM, sources
# and javadoc. The Gradle module file goes too, for Gradle consumers.
FILES=("$ARTIFACT.aar" "$ARTIFACT.pom" "$ARTIFACT.module" "$ARTIFACT-sources.jar" "$ARTIFACT-javadoc.jar")
for file in "${FILES[@]}"; do
  [ -s "$SOURCE/$file" ] || die "missing or empty: $file"
done
# Anything else would be uploaded unsigned and unchecked.
while IFS= read -r found; do
  name="$(basename "$found")"
  case "$name" in
    *.md5 | *.sha1 | *.sha256 | *.sha512 | *.asc) continue ;;
  esac
  printf '%s\n' "${FILES[@]}" | grep -qxF "$name" || die "unexpected file in the release: $name"
done < <(find "$SOURCE" -type f)

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export GNUPGHOME="$WORK/gnupg"
mkdir -m 700 "$GNUPGHOME"

printf '%s\n' "$SIGNING_KEY" | gpg --batch --quiet --import 2>"$WORK/import.log" || {
  cat "$WORK/import.log" >&2
  die "the signing key does not import"
}
SECRET_KEYS="$(gpg --batch --list-secret-keys --with-colons | grep -c '^sec:' || true)"
[ "$SECRET_KEYS" = 1 ] || die "SIGNING_KEY must hold exactly one secret key, and holds $SECRET_KEYS"
FINGERPRINT="$(gpg --batch --list-secret-keys --with-colons | awk -F: '/^fpr:/ { print $10; exit }')"
echo "Signing with $FINGERPRINT"

STAGE="$WORK/bundle"
mkdir -p "$STAGE/$COORDINATE/$VERSION"
for file in "${FILES[@]}"; do
  target="$STAGE/$COORDINATE/$VERSION/$file"
  cp "$SOURCE/$file" "$target"
  printf '%s' "${SIGNING_KEY_PASSWORD:-}" |
    gpg --batch --yes --quiet --pinentry-mode loopback --passphrase-fd 0 \
      --armor --detach-sign --output "$target.asc" "$target"
  gpg --batch --quiet --verify "$target.asc" "$target" 2>/dev/null ||
    die "the signature on $file does not verify"
  md5sum "$target" | cut -d' ' -f1 | tr -d '\n' >"$target.md5"
  sha1sum "$target" | cut -d' ' -f1 | tr -d '\n' >"$target.sha1"
done

rm -f "$BUNDLE"
mkdir -p "$(dirname "$BUNDLE")"
BUNDLE="$(cd "$(dirname "$BUNDLE")" && pwd)/$(basename "$BUNDLE")"
(cd "$STAGE" && zip -qr "$BUNDLE" .)
echo "Bundle: $BUNDLE ($(find "$STAGE" -type f | wc -l | tr -d ' ') files)"
