#!/bin/sh
# Starts offline-protocol-service from environment variables, so a host's
# container manifest configures it without rewriting the command.
#
#   OFFLINE_PROTOCOL_STORE_KEY  required: 64 hex digits, the key the identity
#                               and the queued messages are sealed under
#   OP_CONFIG                   ProtocolConfig JSON (default: the baked file)
#   OP_DATA                     state directory (default /var/lib/offline-protocol)
#   OP_LISTEN                   peer-stream listener (default 0.0.0.0:7878)
#   OP_PEERS                    space-separated static peers, host:port or off1...@host:port
#   OP_LAN                      1 to advertise and discover on the LAN (default 1)
#   OP_HTTP                     HTTP front address (default 127.0.0.1:8080; empty for none)
#   OP_HTTP_ALIASES             device alias file for the front
#   OP_RELAY                    internet relay URL; its token in OFFLINE_PROTOCOL_RELAY_TOKEN
#   OP_SOCKET                   local API socket (default /run/offline-protocol/api.sock)
#
# Any arguments are appended to the command.
set -eu

if [ -z "${OFFLINE_PROTOCOL_STORE_KEY:-}" ]; then
    echo "OFFLINE_PROTOCOL_STORE_KEY is not set: generate one once with 'openssl rand -hex 32' and keep it" >&2
    exit 64
fi

data="${OP_DATA:-/var/lib/offline-protocol}"
set -- --config "${OP_CONFIG:-/etc/offline-protocol/config.json}" \
    --mls-root "$data/mls" --state-root "$data/state" \
    --socket "${OP_SOCKET:-/run/offline-protocol/api.sock}" \
    --listen "${OP_LISTEN:-0.0.0.0:7878}" "$@"

for peer in ${OP_PEERS:-}; do
    set -- "$@" --peer "$peer"
done
if [ "${OP_LAN:-1}" = "1" ]; then
    set -- "$@" --lan
fi
if [ -n "${OP_HTTP-127.0.0.1:8080}" ]; then
    set -- "$@" --http "${OP_HTTP-127.0.0.1:8080}"
fi
if [ -n "${OP_HTTP_ALIASES:-}" ]; then
    set -- "$@" --http-aliases "$OP_HTTP_ALIASES"
fi
if [ -n "${OP_RELAY:-}" ]; then
    set -- "$@" --relay "$OP_RELAY"
fi

exec offline-protocol-service "$@"
