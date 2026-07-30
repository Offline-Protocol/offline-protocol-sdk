#!/usr/bin/env bash
# Guards the hand-maintained license surface against drift.
#
# Three license documents are triplicated into each package that redistributes
# the binaries (root, react-native, python) -- LICENSE and LICENSE-COMMERCIAL.md
# by hand, THIRD-PARTY-NOTICES.md by generate-third-party-notices.sh. Nothing
# else stops an edit from landing in one copy and not the others, and a
# divergent license text in a published artifact is not a bug you find by
# testing. This asserts the copies are byte-identical and that every
# notice-bearing file carries the same copyright line.
set -euo pipefail
cd "$(dirname "$0")/.."

status=0

# Documents copied verbatim into each redistributing package.
for doc in LICENSE LICENSE-COMMERCIAL.md THIRD-PARTY-NOTICES.md; do
  for pkg in bindings/react-native bindings/python; do
    if ! cmp -s "$doc" "$pkg/$doc"; then
      echo "error: $pkg/$doc has drifted from $doc" >&2
      if [ "$doc" = "THIRD-PARTY-NOTICES.md" ]; then
        echo "       run scripts/generate-third-party-notices.sh to resync" >&2
      else
        echo "       copy the root file over it -- all three must match" >&2
      fi
      status=1
    fi
  done
done

# The copyright notice AGPL-3.0 section 4 obliges downstream copies to
# reproduce. Lives beside the license terms rather than inside LICENSE, which
# is the verbatim GPL document and must not be modified.
NOTICE="Copyright © 2025-2026 Offline Protocol, Inc."
for f in \
  README.md \
  LICENSE-COMMERCIAL.md \
  bindings/react-native/README.md \
  bindings/react-native/LICENSE-COMMERCIAL.md \
  bindings/python/README.md \
  bindings/python/LICENSE-COMMERCIAL.md
do
  if ! grep -qF "$NOTICE" "$f"; then
    echo "error: $f is missing the copyright notice: $NOTICE" >&2
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "License documents are consistent across root, react-native, and python."
fi
exit "$status"
