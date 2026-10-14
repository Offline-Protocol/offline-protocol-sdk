#!/bin/sh
# Calls the timeofday service on another device through the HTTP front on
# this host. DEVICE is the device's off1... address, or an alias from the
# front's --http-aliases file.
#
#   ./client.sh off1qx7jj4u8w32ptzysnkadwjzmz9w2nukfmc3ts2ap
#   ./client.sh bob http://127.0.0.1:8080
#
# A front started with --http-token-file requires its token: set
# OP_TOKEN_FILE to that file. It is read at every call, since the front
# writes a new token each time it starts.
#
# A host that resolves *.offline.protocol.internal to the front needs no
# Host header: curl http://timeofday.bob.offline.protocol.internal/now
set -eu

if [ $# -lt 1 ]; then
    echo "usage: $0 DEVICE [FRONT_URL]" >&2
    exit 64
fi
device="$1"
front="${2:-http://127.0.0.1:8080}"

if [ -n "${OP_TOKEN_FILE:-}" ]; then
    # Read on its own line, so a missing file stops the script here rather
    # than sending an empty token.
    token="$(cat "$OP_TOKEN_FILE")"
    set -- -H "X-Offline-Protocol-Token: $token"
else
    set --
fi

curl --silent --show-error --fail-with-body "$@" \
    -H "Host: timeofday.${device}.offline.protocol.internal" \
    "${front}/now"
echo
