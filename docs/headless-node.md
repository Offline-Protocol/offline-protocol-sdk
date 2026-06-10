# Headless node

`offline-protocol-node` runs a full Offline Protocol mesh node — MLS
identity, the capability exchange, transports — as a daemon on servers,
Raspberry Pis, or laptops, and exposes a **localhost HTTP control API**.
This is the bridge that lets off-device software participate in the mesh:
the Capability Exchange MCP server's `node` mode is its primary consumer.

```
agent (Claude etc.) ──MCP──► capability-exchange mcp-server ──HTTP──► offline-protocol-node ──mesh──► peers
                              (budgets, quotes, confirmation)          (identity, signing, transports)
```

The split matters: the node is the trusted mesh participant holding the
identity key and the prepaid ledger; the MCP server in front of it is the
wallet gate (budgets, per-call caps, quote → human confirmation). The
control API itself binds to `127.0.0.1` and supports a bearer token — it is
a local control plane and must never be exposed to a network.

## Running

```bash
NODE_USER_ID=my-node \
NODE_DATA_DIR=/var/lib/offline-node \
NODE_API_TOKEN=$(openssl rand -hex 24) \
NODE_INTERNET_ENABLED=true \
NODE_INTERNET_SERVER=wss://relay.example.com \
cargo run --release --package offline-protocol-node
```

| Env var | Default | Meaning |
|---------|---------|---------|
| `NODE_USER_ID` | `offline-node` | Stable OfflineID identity (set this explicitly — it is the exchange identity on receipts) |
| `NODE_APP_ID` | `capability-exchange` | Protocol application id |
| `NODE_DATA_DIR` | `./node-data` | MLS key material + exchange state (created `0700`; contains key material — protect it) |
| `NODE_BIND` | `127.0.0.1` | Control API bind address |
| `NODE_PORT` | `8990` | Control API port |
| `NODE_API_TOKEN` | — | Bearer token; unset = unauthenticated dev mode (warned at startup) |
| `NODE_INTERNET_ENABLED` | `false` | Enable the Internet transport |
| `NODE_INTERNET_SERVER` | transport default | Internet transport relay address |
| `NODE_LOG` | `info` | `debug` / `info` / `warn` / `error` |

MLS identity and durable exchange state (ledger, receipts, reputation)
persist in `NODE_DATA_DIR` across restarts via a file-backed `MlsStorage`
(atomic temp-file + rename writes; hex-encoded key ids so hostile ids cannot
escape the directory). Published listings are runtime state — re-publish
them after a restart.

## Control API

All bodies are JSON; errors are `{"error": "..."}`. With `NODE_API_TOKEN`
set, every route except `GET /healthz` requires `Authorization: Bearer …`.
Response shapes serialize the `offline-protocol-exchange` structs directly,
so they match the TypeScript wire types in `@capability-exchange/core`.

| Route | Does |
|-------|------|
| `GET /healthz` | `{ok, user_id, state}` |
| `POST /v1/discover` `{service_id?}` | Broadcasts a discovery query; returns `{query_id}`. Results accumulate in the listing cache. |
| `GET /v1/listings` | Cached `DiscoveredListing[]` (listing, attestation_status, hop_count, provider_peer_id, last_seen_ms) |
| `POST /v1/invoke` `{provider, service_id, method, body, max_units?, timeout_ms?}` | Invokes a discovered listing and **waits** for the mesh response (and, for priced listings, the signed receipt). Returns `{request_id, status, body, receipt?}`. Priced invocations enforce all on-node exchange rules: verified attestation, confirmed MLS session, prepaid hold. |
| `POST /v1/adapters/pull` `{provider, service_id, timeout_ms?}` | Pulls an adapter and waits; returns the hash-verified bytes `{content_hash, size_bytes, data_base64}` or a rejection error. |
| `GET /v1/balance?currency=USD` | On-mesh prepaid ledger `{available_minor, held_minor}` |
| `POST /v1/balance/credit` `{currency, amount_minor}` | Credits the mesh ledger (after clearing-service funding) |
| `GET /v1/receipts/pending` | `UsageReceipt[]` awaiting settlement |
| `POST /v1/receipts/mark-settled` `{receipt_ids}` | Marks receipts settled after the clearing service confirms |
| `POST /v1/listings/publish` `{service_id, version, capabilities, kind, terms}` | Publishes an attested listing signed with the node's MLS identity key |
| `POST /v1/adapters/publish` `{…, base_model, base_model_version, artifact_path}` | Publishes an attested adapter from a local artifact file (hash computed and signed; pulls auto-served) |

Long-polling semantics: `invoke` and `adapters/pull` block until the
corresponding mesh event arrives or `timeout_ms` (default 30 s, max 120 s)
elapses — the node turns the protocol's event stream into synchronous HTTP
so clients stay simple.

## Transports

The headless node enables the **Internet** transport (relay-based) by
configuration; BLE and WiFi Direct are platform-bound (mobile) and not
available to a daemon. Reticulum/Nostr wiring follows the same
`transport_manager_mut().add_transport(...)` pattern if needed.
