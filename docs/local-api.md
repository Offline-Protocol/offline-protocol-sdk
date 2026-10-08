# The local API: running the SDK as a service

The SDK is usually embedded: one application, one process, one engine. The
local API is the other shape. One process owns the engine and serves any
number of local applications over a socket, so a machine with several
programs that all want the same identity, the same peers and the same
documents runs one node instead of several. Each application talks JSON-RPC
2.0 over a WebSocket, declares which application it is, and gets the
engine's own methods and events by name.

This page is the guide: how to start the service, how a client talks to it,
and what the two shipped examples show. The contract, with every method,
event and error, is [the local API chapter](spec/local-api.md); what the
reference server owes is in [the bridge contract](bridges/local-api.md).

## What holds, whatever the client does

Four things are true of every service, and a client written against them
does not break when the server changes:

- **The server owns the engine.** It runs the loop, drains inbound messages,
  starts and stops. No client can call `process()`, `receive_message()` or
  `stop()`, because two drainers would split one inbound stream and a
  stopped engine would leave every other application's messages
  unacknowledged.
- **A connection speaks for one application.** The first request is
  `hello` with an `app_id`. Every message that connection sends is stamped
  with it, and an inbound message stamped with it reaches only connections
  that declared it. Two processes of the same application declare the same
  id and are one application to every rule.
- **The socket is the authorization boundary.** Whoever can open the Unix
  socket, or holds the launch token on TCP, is one of the operator's
  applications. The server does not tell one local process from another,
  and the rules below separate applications from each other's mistakes, not
  from a hostile process on the same account.
- **Events are the engine's JSON, unchanged.** A client that handles
  `message_received` from the embedded SDK handles the same object here.

## Starting the service

The Python package ships the reference server as the
`offline-protocol-service` command. It runs over the built-in file stores,
so a host with no secret service needs only a directory and a key:

```bash
export OFFLINE_PROTOCOL_STORE_KEY="$(openssl rand -hex 32)"   # once; keep it
offline-protocol-service --config config.json \
    --mls-root /var/lib/example/keys --state-root /var/lib/example/state \
    --socket /run/example/api.sock \
    --listen 0.0.0.0:7878 --peer 10.0.0.2:7878
```

`config.json` holds the `ProtocolConfig` fields by name (see
[Configuration](configuration.md)); `--listen` and `--peer` drive the
peer-stream transport when the configuration enables it, and `--gateway
HOST:PORT` names the gateway daemon when it enables `reticulum` (default
`localhost:4242`, see [the gateway contract](spec/gateway-contract.md)).

### Which transports run

The server starts every transport the configuration enables and the host
can run, once the engine has an address to prove:

| Transport | Enabled by | Started when |
|---|---|---|
| Peer stream | `wifi_direct_enabled` | Always. `--listen`, `--peer` and `--lan` configure it. |
| Bluetooth LE central and peripheral | `ble_enabled` | The central always; the peripheral where `bless` has a backend (macOS, and Linux through BlueZ). The peripheral advertises this device to phones. |
| Internet relay | `internet_enabled` | `--relay URL` names the relay; the token, if any, is read from `OFFLINE_PROTOCOL_RELAY_TOKEN` (`--relay-token-env` names another variable). |
| Gateway | `reticulum_enabled` | Always once configured; `--gateway` names the daemon. |

`--lan` advertises this host on the LAN over DNS-SD and dials every other
host that advertises the same service type, so two hosts on one segment find
each other with no `--peer` list. It needs the optional dependency
(`pip install 'offline-protocol-sdk[lan]'`), and multicast DNS, so a
container must share the host's network. An address a record carries is a
hint; the stream's preamble proves the peer.

**A transport that fails to start is logged and stays stopped, and the
others run; the server fails only when none starts.** The failure this
prevents is a Bluetooth adapter another process holds, or a refused
advertisement, taking the LAN path down with it. A transport that has not
started within 30 seconds counts as failed and is stopped then, not at
shutdown, so a Bluetooth backend that hangs cannot keep the API socket from
opening, nor finish starting later with nothing watching it. Transports start
one after another, so the socket can take up to 30 seconds per configured
transport to open. A flag the configuration
cannot honour (`--lan` without `wifi_direct_enabled`, `--relay` without
`internet_enabled`) is refused at start rather than ignored, and so is a
value the transport could never use: `--relay` must be a `ws://` or `wss://`
URL with a host, because the internet transport retries a failed connect
for as long as it runs and would never report the mistake, and a variable
named with `--relay-token-env` must be set. A `ws://` relay that is not on
loopback is accepted with a warning: the token crosses the network in clear.

Two carriers serve the API itself:

| Carrier | How | Credential |
|---|---|---|
| Unix domain socket (default) | `--socket PATH` | The socket file is `0600` in a `0700` directory. `hello` carries no token. |
| Loopback TCP (opt-in) | `--tcp PORT --token-file PATH` | A 32-byte token, new at every launch, written to the file with mode `0600` in the same call that creates it. `hello` carries it. |

The server never binds a non-loopback address. A deployment that wants the
API across a network puts a reverse proxy with its own authentication in
front. `GET /health` on either carrier answers with the server's name and
version, the API version and the carrier, and nothing else is served over
plain HTTP: the
pinned WebSocket library accepts only `GET` and drops any other method
before the server sees it, so a `POST` would fail silently rather than with
an error.

## The HTTP front

An application that should not know the SDK exists can still reach a service
on another device: `--http 127.0.0.1:8080` (with the `http` extra) starts the
[HTTP front](spec/http-front.md) in the same process, as a client of this
server under the application id `offline-protocol-http-front`. A policy
file that names any application must list that id too.

A provider registers its HTTP endpoint with the front on its own host:

```bash
curl -X PUT http://127.0.0.1:8080/services/timeofday \
    -H 'Content-Type: application/json' \
    -d '{"callback": "http://127.0.0.1:9000", "version": "1"}'
```

A requester on another host calls it by device address, or by an alias from
`--http-aliases`:

```bash
curl http://127.0.0.1:8080/now \
    -H 'Host: timeofday.bob.offline.protocol.internal'
```

The request travels inside an end-to-end encrypted message, the far front
calls `http://127.0.0.1:9000/now` with `X-Offline-Protocol-Sender` set to the
caller's authenticated address, and the answer comes back as the response.
`GET /browse/timeofday` streams the devices that offer a service, as
server-sent events. Registrations are kept beside the state root and
registered again at every start. A host that resolves
`*.offline.protocol.internal` to the front removes the need for the manual
`Host` header; until then, setting it by hand exercises the same path. The
front runs apart from the service as `offline-protocol-http-front`, never
beside `--http` on the same service: both would receive every request and
call the callback twice. Off loopback it requires `--http-token-file`, and
on a host where a browser runs it should have one on loopback too; the
residual risk of a front any local process can reach is R23 in the threat
model.

## The policy file

With no policy, any well-formed application id is accepted and nothing is
denied. `--policy policy.json` adds rules per application id:

```json
{
  "spaces": {"notes": ["notes-*"], "mail": ["mail-*", "shared"]},
  "denied": {"kiosk": ["sign_data", "manual_mls", "tuning"]},
  "applications": ["admin"]
}
```

| Section | What it does | The failure it prevents |
|---|---|---|
| `spaces` | Glob patterns over document space ids. A `data.*` call outside them is refused, `data.list_spaces` is filtered, and document events for other spaces are not delivered. | Two applications picking the same space name by accident and merging each other's documents; the engine cannot notice, because to it there is one member. |
| `denied` | Method groups (`sign_data`, `manual_mls`, `tuning`) or single wire names an application may not call. Refused with `PermissionDenied`, not "method not found", so the client can read the refusal. | Every application holding a signing oracle over the shared identity key. |
| `applications` | Ids with no rule of their own that are still admitted. | An application locked out by the rule below because it needs no restriction. |

**Once `spaces` or `denied` names anyone, an id no section names is refused
at `hello`.** Without that, an application scoped or denied under one id
would reconnect under another and be scoped and denied nothing. Service
ownership (which application registered which service) is a runtime record
the server rebuilds from the clients' own calls, and it never turns this
rule on.

## Talking to it

Every request is a JSON-RPC 2.0 object with an `id`; every event is a
notification with `method` set to `event` and `params` set to the event
object itself.

**`hello` first.** It declares the application, and on TCP carries the
token. The result has the API version, the server's name and version, the
engine's state and the node's `off1…` address, which every client of one
server shares.

```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{"app_id":"notes","client":"notes-desktop/2.4"}}
{"jsonrpc":"2.0","id":1,"result":{"api_version":1,"server":{"name":"offline-protocol-service","version":"0.28.0"},"state":"Running","local_address":"off1…"}}
```

**Sending.** The engine's methods by name, with the parameters the interface
definition gives them. `send_message` returns the message id; the
`message_sent` and `message_delivered` events that name it come back to the
connection that sent it and to no other, while the server that issued the
id is running. A message the engine gives up on is reported the same way,
as `message_undeliverable`, so a client watches for both.

```json
{"jsonrpc":"2.0","id":2,"method":"send_message","params":{"recipient":"off1…","content":"hi","priority":"Medium"}}
```

**Subscribing and replay.** After `hello` a connection receives every event
the routing rules deliver to its application. `subscribe` with a list of
tags narrows that, `unsubscribe` widens it back, and `subscribe` with
`"all"` resets it (`unsubscribe` with `"all"` mutes everything). An
inbound message for an application with no connected client is held, up to
256 per application id with the oldest dropped past that, and delivered
right after the next `hello` under that id. Delivery is at least once, so
`message_id` is the key a client deduplicates on. The engine has already
acknowledged a held message, which is why the server holds it rather than
dropping it: the sender will not send it again.

**Documents.** The replicated document store is reachable as `data.*`
methods, and the mesh services as `services.*`. A write carries a tagged
value; a read returns plain JSON:

```json
{"jsonrpc":"2.0","id":3,"method":"data.create_doc","params":{"space_id":"notes-1","doc_id":"todo"}}
{"jsonrpc":"2.0","id":4,"method":"data.map_set","params":{"space_id":"notes-1","doc_id":"todo","collection":"fields","key":"title","value_json":"{\"kind\":\"text\",\"value\":\"groceries\"}"}}
{"jsonrpc":"2.0","id":5,"method":"data.doc_json","params":{"space_id":"notes-1","doc_id":"todo"}}
{"jsonrpc":"2.0","id":5,"result":"{\"fields\":{\"title\":\"groceries\"}}"}
```

`create_doc` on a document that exists is a no-op, so a client need not
check first. When the data layer is off in the configuration, every
`data.*` call answers `DataDisabled`.

**One-shot calls.** There is no HTTP request path (see above). A single
call is a connection that sends `hello`, one request, and closes; every
runtime has a WebSocket client, so this is the whole "curl-shaped" case.

**Errors.** A failed call is a JSON-RPC error whose `data.variant` is the
engine's own `ProtocolError` variant name. Switch on that, not on the
number: the number is the variant's position in the append-only error enum,
stable but not meaningful.

```json
{"jsonrpc":"2.0","id":6,"error":{"code":-32004,"message":"No key package available for recipient: off1…","data":{"variant":"NoKeyPackage"}}}
```

A method before `hello` is `InvalidState`; a bad application id is
`InvalidArgument`; a wrong token, another application's service, a space
outside the allow-list, a denied method, or an unlisted id once rules exist
is `PermissionDenied`. There is no error that exists only on the wire.

## What is not exposed

The interface definition has two kinds of method. The ones an application
calls are on the wire. The ones a platform bridge calls to drive a radio,
attach storage, feed the engine a frame, run the loop, or own telemetry are
not, because they belong to whoever owns the engine, and here that is the
server. The chapter lists both sets in full, and a test in the Rust
workspace holds the lists, the interface definition and the server's
dispatch table to one another, so a method added to the engine is a failing
test until someone says which side of the line it is on.

## The examples

Both examples do the same thing, in the same order: `hello`, `subscribe`,
`send_message` and the `message_delivered` event for it (when given an
address), a document edit read back, and a one-shot call on a fresh
connection.

- [`bindings/python/examples/local_api_client.py`](../bindings/python/examples/local_api_client.py):
  Python over the Unix socket or TCP, with `--wait` to sit and receive one
  inbound message. Needs only the `websockets` package the SDK depends on.
- [`examples/local-api/client.mjs`](../examples/local-api/client.mjs): Node
  22 or later with the built-in `WebSocket`, no dependencies. The built-in
  client dials only `ws:` and `wss:` URLs, so it uses the TCP carrier and
  reads the token file.

A test in the Python suite runs the Python example against an in-process
server, and runs the Node example end to end when `node` is on the path, so
a change to the wire that breaks either is caught before it ships.
