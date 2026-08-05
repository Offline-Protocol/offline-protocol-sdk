#!/usr/bin/env bash
# Asserts the vendored NIP-44 test vectors are the official ones.
#
# The NIP-44 unit tests are the only thing standing between a subtly wrong
# encrypt-then-MAC implementation and silent interop failure with every other
# Nostr client. Those tests read a vendored copy of `nip44.vectors.json`, and a
# vendored fixture is exactly the kind of file an edit can slip into: adjusting
# a vector to match a broken implementation turns a red suite green and leaves
# no trace in the diff that a reviewer would recognise as a crypto change.
#
# The NIP-44 spec publishes a sha256 of the vector file for this reason. This
# pins the vendored copy to that checksum, so the fixture can only change by
# also changing the value below -- a one-line diff that reads as what it is.
#
# If upstream legitimately republishes the vectors, update BOTH the file and
# EXPECTED_SHA256, and re-read the spec's "Tests and code" section: the vectors
# and the spec body have drifted apart before (see the extended-length-prefix
# note in crates/offline-protocol-transport/src/nip44.rs).
set -euo pipefail
cd "$(dirname "$0")/.."

# From nips/44.md, "Tests and code". Verified 2026-08-05.
EXPECTED_SHA256="269ed0f69e4c192512cc779e78c555090cebc7c785b609e338a62afc3ce25040"
VECTORS="crates/offline-protocol-transport/tests/data/nip44.vectors.json"

if [ ! -f "$VECTORS" ]; then
  echo "error: $VECTORS is missing" >&2
  exit 1
fi

actual="$(shasum -a 256 "$VECTORS" | cut -d' ' -f1)"

if [ "$actual" != "$EXPECTED_SHA256" ]; then
  echo "error: $VECTORS does not match the checksum published in NIP-44" >&2
  echo "       expected: $EXPECTED_SHA256" >&2
  echo "       actual:   $actual" >&2
  echo "       The vectors are a crypto conformance fixture, not a test file to" >&2
  echo "       adjust. If upstream republished them, update the file and the" >&2
  echo "       EXPECTED_SHA256 in this script together." >&2
  exit 1
fi

echo "NIP-44 vectors match the published checksum ($EXPECTED_SHA256)"
