#!/usr/bin/env bash

# Print the Python package version for a release version.
#
# A release is numbered once, by its tag, in semantic-versioning form, and
# Python spells a prerelease differently: the tag v0.28.0-rc.1 is the wheel
# 0.28.0rc1. pyproject.toml carries the release core (0.28.0), which the
# version gate compares against the tag's core, so a wheel built from it as it
# stands would be numbered as the final release. On PyPI that number would
# then be gone: a version there can never be uploaded again, even after a
# delete, so a release candidate built that way would burn the release it
# rehearses.
#
# Strict on purpose. A suffix this does not know has no Python spelling we
# have agreed on, and a guess is a published number nobody chose.
#
# Usage:
#   bash scripts/pep440-version.sh 0.28.0-rc.1    # prints 0.28.0rc1

set -euo pipefail

[ "$#" -eq 1 ] || {
  echo "usage: $0 <semver>" >&2
  exit 2
}

SEMVER="$1"
CORE='([0-9]+)\.([0-9]+)\.([0-9]+)'

if [[ "$SEMVER" =~ ^${CORE}$ ]]; then
  echo "$SEMVER"
elif [[ "$SEMVER" =~ ^${CORE}-(rc|alpha|beta)\.?([0-9]+)$ ]]; then
  case "${BASH_REMATCH[4]}" in
    rc) pre=rc ;;
    alpha) pre=a ;;
    beta) pre=b ;;
  esac
  echo "${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}$pre${BASH_REMATCH[5]}"
elif [[ "$SEMVER" =~ ^${CORE}-dev(\.([0-9]+))?$ ]]; then
  echo "${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.dev${BASH_REMATCH[5]:-0}"
else
  # Build metadata (+...) would be a local version, which PyPI refuses.
  echo "ERROR: $SEMVER has no Python spelling: use X.Y.Z, X.Y.Z-rc.N, X.Y.Z-alpha.N, X.Y.Z-beta.N or X.Y.Z-dev[.N]" >&2
  exit 1
fi
