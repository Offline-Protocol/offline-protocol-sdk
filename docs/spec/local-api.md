# The local API

## What this chapter is for

Every binding in this repository puts the engine inside the application's own
process: a Swift or Kotlin bridge, a Python module, a React Native package.
One application, one process, one engine. A long-lived host that serves
several local applications at once, on a desktop, a server, or a headless
box, has no shape to run the engine in, because nothing outside the process
can reach it.

This chapter specifies that shape: one **server** process owns one engine
instance, and any number of local **clients** reach it over a socket. The
wire between them is JSON-RPC 2.0 over a WebSocket. A client written against
this chapter, in any language, works against any conforming server, and a
server written against it serves any conforming client. The reference server
ships in the Python package; it is one implementation of this contract, not
the contract.

Three things this chapter deliberately does not do. It does not invent a
second API: every method is one of the interface definition's own methods
under its own name, with its own parameters, returning its own result, and
every event is the engine's own event JSON, relayed unchanged. It does not
change anything on the mesh: nothing here reaches another device, and a peer
cannot tell whether it is talking to an application with an embedded engine
or to a server fronting six of them. And it does not add a second identity:
one server is one identity, and the applications behind it share it by
construction. What separating them requires is stated in the invariants, and
it is the server's job, never the wire's.

## Invariants

Six things hold for every server and every client. A server that keeps the
mechanisms below and breaks one of these is not a conforming server.

- **The server owns the run loop and the drain.** The server calls
  `process()` on its timer and drains `receive_message()` on every tick; no
  client ever does either, and neither is a method on the wire. This is
  load-bearing rather than tidy: the engine emits `message_received` from
  inside the drain, so a message is delivered, acknowledged and dedup-marked
  only when something drains. Two drainers would split one stream between
  them, and no drainer would leave every inbound message unacknowledged
  forever while the sender retries into a black hole.
- **A connection sends as the application it declared.** The first request
  on a connection is `hello`, which names an application id, and the server
  stamps that id on every message and media transfer sent over that
  connection, through the per-send application id the send options carry,
  so a frame on the mesh names the application it came from and the
  receiving side can route it. A connection cannot send as a different
  application from the one it declared, and a server that let it would
  defeat every routing rule below. The id is self-declared and it is
  metadata, never an access control
  ([R17](../security/threat-model.md#r17-the-application-id-on-every-frame-is-cleartext-and-unsigned)):
  what stops a local process from declaring another application's id is the
  socket boundary and the unlisted-id rule below, not the wire.
- **An event reaches the clients it is for.** An event that carries an
  application id reaches only the clients that declared that id. An event
  that names an identifier the server handed to one client reaches that
  client. Everything else is engine-level and reaches every client. A server
  never delivers a stamped or correlated event to a client it is not for,
  because the alternative is every application reading every other
  application's messages through the side door of a shared socket.
- **The socket is the authorization boundary.** Whoever can open the socket
  is one of the operator's applications. There is no user database, no
  session token that outlives a launch, and no permission model on the
  wire. A Unix domain socket with owner-only permissions is that boundary by
  construction; a TCP listener on loopback is that boundary only with the
  per-launch token, and is never bound to anything but loopback. Inside the
  boundary sit root and every process running as the owner's uid, and the
  server does not check peer credentials, so nothing below distinguishes
  one of those processes from another. Application ids are self-declared,
  and the server-side rules (which application owns a service, which spaces
  one may open, which ids are listed) separate the applications from each
  other's mistakes: a name collision, a space opened by accident, a client
  started with the wrong id. They do not separate them from a hostile
  local application, which is inside the boundary by definition. Every one
  of those rules is applied before a call reaches the engine, and none
  appears on the mesh ([data replication](data-sync.md) invariant: a space
  is an MLS scope with no second membership system).
- **The platform operations are never on the wire.** The interface
  definition has two kinds of method: what an application calls, and what
  the platform layer that drives a radio, a relay socket or the run loop
  calls. The second kind (every `ble_*`, `internet_*`, `wifi_direct_*`,
  `reticulum_*` and `nostr_*` operation, every callback installer, the
  lifecycle, the storage attach, the telemetry pipe and the host feeds) is
  the server's own and is not reachable from a client under any name. A
  client that could inject an inbound frame or announce a peer would hold the
  same authority the threat model's boundary 2 exists to keep on the far
  side of a dedicated entry point.
- **Nothing here is a second implementation of anything.** Method names,
  parameter names, result shapes, enum spellings, event tags and error
  variants are the interface definition's. The server maps a JSON request to
  the generated binding's call and the binding's result to JSON; it does not
  reimplement, re-validate, or re-encode a message. The failure this
  prevents is the one every hand-written bridge has had: a field renamed in
  one copy and not the other, invisible until a device disagrees.

## Framing

### JSON-RPC 2.0 over one WebSocket

A connection is one WebSocket. Every message on it is one JSON-RPC 2.0
object as a text frame: a request from the client, a response from the
server, or a notification from the server. A client never sends a
notification, and a server never sends a request.

```
--> {"jsonrpc":"2.0","id":1,"method":"hello","params":{"app_id":"notes"}}
<-- {"jsonrpc":"2.0","id":1,"result":{"api_version":1,"server":{"name":"offline-protocol-service","version":"0.28.0"},"state":"Running","local_address":"off1..."}}
--> {"jsonrpc":"2.0","id":2,"method":"send_message","params":{"recipient":"off1...","content":"hi","priority":"Medium","reply_to_msg":null}}
<-- {"jsonrpc":"2.0","id":2,"result":"5f0c..."}
<-- {"jsonrpc":"2.0","method":"event","params":{"type":"message_delivered","message_id":"5f0c...","latency_ms":412,"hop_count":1,"transport":"ble"}}
```

- **Requests carry `params` by name**, as a JSON object whose keys are the
  method's parameter names. Positional `params` are refused with
  `-32602`. A method with no parameters takes `{}` or no `params` member.
- **The request `id`** is a string or a number; the response carries it back
  unchanged. Responses correlate by `id` and MAY arrive in a different order
  from the requests, so a client that pipelines keeps its own map. A request
  with no `id` is a JSON-RPC notification, which a client never sends; the
  server drops it without a response.
- **Events are notifications** with `method` set to `event` and `params`
  set to the event object itself, exactly as the engine serialised it, with
  its `type` tag. Nothing is added, removed or renamed
  ([C3](../bridges/README.md#c3-events-cross-as-opaque-json)).
- **Batches** (a JSON array of requests) are not supported in this version.
  A server answers one with `-32600`.
- **One request in flight per method call**: the server MAY execute requests
  from one connection concurrently or in order, and a client MUST NOT depend
  on either. The engine serialises calls on its own lock regardless.
- **A one-shot call is a connection that sends `hello`, one request, and
  closes.** Every mainstream runtime carries a WebSocket client, so this is
  the whole of the "curl-shaped" use case, and it is why no second protocol
  is needed for it.

### Why there is no HTTP

An earlier draft of this chapter served plain HTTP `POST` on the same port
for one-shot calls, through the WebSocket library's request hook. That was
verified before this chapter was written against `websockets` 16.1, the
version the Python package's lock file pins (the package itself accepts any
release from 12 up to but not including 17, so 16.1 is the only version the
behaviour has been checked on), and it does not work: the library's
handshake parser accepts only `GET`, and a connection that opens with any
other method is closed with no HTTP response at all, before the hook runs. A
`POST` would fail with zero bytes back rather than with a JSON-RPC error,
which is the worst possible failure for the caller this feature was meant to
serve. So there is no HTTP request path in this contract, and a server MUST
NOT advertise one; a client that wants a single call opens a WebSocket for
it. The reference server pins the fact with a test, so a library upgrade
that changes it is noticed rather than assumed.

What the hook can do is answer a plain `GET` that does not ask for an
upgrade. A server MAY serve `GET /health` that way, answering `200` with a
small JSON body:

```json
{"server":{"name":"offline-protocol-service","version":"0.28.0"},"api_version":1,"carrier":"unix"}
```

It is unauthenticated because it reveals nothing beyond the fact that a server
is listening, which the open port already reveals. It carries no engine state,
no address and no client count. A server that does not implement it answers
the library's default (`426 Upgrade Required`), and a client MUST NOT depend
on it.

### Carriers

**A Unix domain socket is the default.** The server creates it with owner-only
permissions (`0600`) inside a directory only the owner can enter (`0700`),
and removes a stale socket file at the same path before binding. The path is
the operator's choice; the reference server documents its default. On this
carrier `hello` carries no token, because the file permissions are the
credential.

**TCP on loopback is opt-in and carries a token.** When an operator enables
it, the server binds `127.0.0.1` or `::1` only, generates 32 random bytes at
launch, writes them hex-encoded to a file at a path the operator names, and
requires them in `hello.token` on every connection. The file is created with
mode `0600` in the same call that creates it, never created and then
narrowed, because the moment between the two is exactly the window the mode
exists to close. The token is regenerated at every launch, so a token read
once never outlives the server that issued it. On neither carrier does the
server check the peer's credentials: the socket permissions and the token
are the whole of the check, and root or any process of the owner's uid
passes it. A `hello` without the token, or
with the wrong one, is answered with `PermissionDenied` and the connection is
closed with WebSocket close code `1008`. A server MUST NOT bind a non-loopback
address in this version; a deployment that wants the API across a network
puts a reverse proxy with its own authentication in front, and that proxy is
outside this contract.

**Frame size.** The WebSocket library's default limit on one message is
1 MiB. `send_media` and `send_file` carry the file bytes inside the request,
so a server MUST raise its inbound limit to at least the engine's file size
limit (100 MiB, and not configurable through the interface today) plus
base64 overhead, or every media send above 1 MiB fails as a closed connection
instead of as the engine's own refusal. A client's outbound limit is its own
business.

## The session

### `hello`

The first request on every connection MUST be `hello`, with one exception:
the three instance-less functions at the end of the method table need no
engine and no application id, and MAY be called before it. Any other method
before `hello` is refused with `InvalidState`, and a second `hello` on the
same connection is refused the same way. A server that answered another
method first would have no application id to stamp on it.

```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{
  "app_id": "notes",
  "client": "notes-desktop/2.4",
  "token": "…"
}}
```

| Parameter | | |
|---|---|---|
| `app_id` | required | The application this connection speaks for. Validated by the same rule as the engine's own `app_id`: at most 256 bytes, not empty, not `.` or `..`, no control character, no `/`, `\` or `:` ([wire format](wire-format.md#the-abstract-message)). An id that fails it is `InvalidArgument`. |
| `client` | optional | A free-form name and version for the server's log. Never on the mesh. |
| `token` | required on TCP | The per-launch token. Ignored on a Unix socket. |

The result:

| Field | |
|---|---|
| `api_version` | `1`. A client that needs something this version does not have checks here and refuses to proceed rather than probing. |
| `server` | `{"name","version"}` of the server implementation. |
| `state` | The engine's `ProtocolState`: `Stopped`, `Running` or `Paused`. A server MAY accept clients before `start()`; every method that needs a running engine then answers `NotStarted`, which is the same answer an embedded caller gets. |
| `local_address` | The identity's `off1…` address, or `null` before the identity exists. Every client of one server gets the same address, which is the one-identity rule made visible. |

Several clients MAY declare the same application id, at once or in sequence;
a second process of the same application is the ordinary case. They are one
application to every rule below: an event for that id reaches all of them,
and any of them may unregister a service the other registered. Whether an
id the server has never seen is accepted depends on whether the operator
has configured a space allow-list or a method deny; see the unlisted-id
rule under server-side rules.

### `subscribe` and `unsubscribe`

After `hello`, a client receives every event the routing rules deliver to
it. `subscribe` narrows that to a list of tags, and `unsubscribe` widens it
back:

```json
{"jsonrpc":"2.0","id":3,"method":"subscribe","params":{"types":["message_received","message_delivered"]}}
{"jsonrpc":"2.0","id":4,"method":"unsubscribe","params":{"types":["message_delivered"]}}
{"jsonrpc":"2.0","id":5,"method":"subscribe","params":{"types":"all"}}
```

The filter is the client's own convenience and changes nothing about
routing: a tag a client subscribed to still reaches it only if the rules
below deliver it. A tag the server does not know is accepted and never
fires, so a client built against a newer engine keeps working against an
older server. Both return `true`.

### Session methods

These three are the server's own and are not in the interface definition.
They are the only such methods; everything else on the wire is one of the
engine's.

| Method | Params | Result |
|---|---|---|
| `hello` | `app_id`, `client?`, `token?` | the object above |
| `subscribe` | `types` (`[]` or `"all"`) | `true` |
| `unsubscribe` | `types` (`[]` or `"all"`) | `true` |

## Encoding

The interface definition's types map to JSON one way, in both directions,
and this is the whole of the mapping:

| Interface type | JSON |
|---|---|
| `string`, `string?` | string; an optional one may be `null` or omitted |
| `boolean` | `true` / `false` |
| `u8`, `u16`, `u32`, `u64`, `i16`, `i64`, `f32`, `double` | number. A `u64` above 2^53 loses precision in a JavaScript client; none of the engine's counters and timestamps reach it in practice, and a client that must be exact parses the raw text |
| enums (`MessagePriority`, `TransportType`, `ProtocolState`, `ContentType`, …) | the definition's own spelling as a string: `"Medium"`, `"WiFiDirect"`, `"Running"`, `"Image"` |
| `sequence<u8>`, `bytes` | a base64 string (RFC 4648, standard alphabet, padded). This is the one place the wire departs from the events, which carry bytes as the engine serialises them (see the catalogue) |
| `sequence<string>` | array of strings |
| `record<DOMString, string>` | object |
| dictionaries (`SendMessageOptions`, `DorsConfig`, `MlsWelcomeMessage`, …) | object with the dictionary's field names. A field with a default in the definition may be omitted and takes that default; a server passes only the fields the caller sent and never substitutes a literal of its own ([C6](../bridges/README.md#c6-config-parsers-must-not-default-to-literals)) |
| `void` | `null` |

The legend used in the tables below: a bare name is a string; `#` is a
number; `!` a boolean; `b64` bytes as base64; `[]` an array of strings;
`{Name}` an object shaped as the named dictionary; `<Enum>` a string from the
named enum; a trailing `?` is optional.

## Method table

Every method below is one of the interface definition's own, called with its
own parameters. The three objects of the definition keep their own prefix on
the wire, so the guard that pins this table can map every row back to its
declaration: engine methods are bare, `MeshServices` methods carry
`services.`, and `DataStore` methods carry `data.`. The instance-less
functions of the definition's namespace are bare too, and are the only
methods a client may call before `hello`.

### Engine: identity and state

| Method | Params | Result |
|---|---|---|
| `get_state` | | `<ProtocolState>` |
| `local_address` | | string or `null` |
| `get_identity_public_key` | | `b64` (32 bytes) |
| `derive_user_id_from_public_key` | `public_key b64` | string |
| `sign_data` | `data b64` | `b64` (64 bytes). Signs with the shared identity key, which every client of one server shares by construction; an operator who does not want every application to hold a signing oracle denies it by application id (see server-side rules) |
| `verify_signature` | `public_key b64`, `data b64`, `signature b64` | `!` |
| `create_invite` | `petname?`, `sign !` | string (the invite blob) |
| `resolve_username` | `username` | `!` |

### Engine: messaging

| Method | Params | Result |
|---|---|---|
| `send_message` | `recipient`, `content`, `priority <MessagePriority>`, `reply_to_msg?` | string (message id). Stamped with the connection's application id; the server calls the rich surface with only that option set |
| `send_message_rich` | `recipient`, `content`, `options {SendMessageOptions}` | string (message id). `options.app_id` is set by the server to the connection's id; a value the client sends is refused with `InvalidArgument` |
| `forward_message` | `original_message_json`, `new_recipient`, `priority <MessagePriority>?` | string (message id) |
| `send_presence_update` | `recipient`, `status <PresenceStatus>` | string (message id) |
| `send_typing_indicator` | `recipient`, `conversation_id`, `is_typing !` | string (message id) |
| `send_read_receipt` | `recipient`, `message_ids []` | string (message id) |

### Engine: connection requests

| Method | Params | Result |
|---|---|---|
| `send_connection_request` | `recipient`, `sender_name`, `key_package b64?`, `initial_message?` | string (message id) |
| `accept_connection_request` | `recipient`, `accepter_name`, `key_package b64?` | string (message id) |
| `reject_connection_request` | `recipient` | string (message id) |
| `cancel_connection_request` | `recipient` | string (message id) |

### Engine: media and files

| Method | Params | Result |
|---|---|---|
| `send_media` | `recipient`, `file_data b64`, `file_name`, `content_type <ContentType>`, `media_metadata {MediaMetadata}?` | string (file id). Stamped like `send_message` |
| `send_media_rich` | `recipient`, `file_data b64`, `file_name`, `content_type <ContentType>`, `options {MediaSendOptions}` | string (file id). `options.app_id` as for `send_message_rich` |
| `send_file` | `recipient`, `file_data b64`, `file_name` | string (file id) |
| `get_file_progress` | `file_id` | `{FileProgress}` or `null` |
| `cancel_file_transfer` | `file_id` | `null` |

### Engine: secure sessions

| Method | Params | Result |
|---|---|---|
| `is_mls_initialized` | | `!` |
| `has_pending_key_package` | `peer_id` | `!` |
| `get_establishment_state` | `peer_id` | `<EstablishmentState>` |
| `establish_secure_session` | `peer_id` | `{MlsWelcomeMessage}` or `null` |
| `rekey_session` | `peer_id` | `!` |
| `mls_has_session` | `other_user_id` | `!` |
| `mls_list_sessions` | | `[]` |
| `mls_delete_session` | `other_user_id` | `null` |

### Engine: manual MLS

The manual surface operates on the shared identity's sessions, exactly as it
does for an embedded application. A client that drives it is driving every
other client's sessions too.

| Method | Params | Result |
|---|---|---|
| `mls_generate_key_package` | | `{MlsKeyPackageBundle}` |
| `mls_get_or_create_key_package` | | `{MlsKeyPackageBundle}` |
| `mls_import_key_package` | `user_id`, `key_package_data b64` | `null` |
| `mls_get_pending_key_packages` | | `[{MlsKeyPackageBundle}]` |
| `mls_mark_key_package_synced` | `package_id` | `null` |
| `mls_create_session` | `other_user_id` | `{MlsWelcomeMessage}` |
| `mls_join_session` | `welcome {MlsWelcomeMessage}` | `{MlsGroupInfo}` |
| `mls_encrypt_for_user` | `other_user_id`, `plaintext b64` | `{MlsEncryptedMessage}` |
| `mls_decrypt_from_user` | `encrypted {MlsEncryptedMessage}` | `b64` or `null` |
| `mls_get_pending_welcome` | `other_user_id` | `{MlsWelcomeMessage}` or `null` |
| `mls_clear_pending_welcome` | `other_user_id` | `null` |
| `mls_decrypt` | `encrypted {MlsEncryptedMessage}` | `b64` or `null` |
| `mls_process_welcome` | `welcome {MlsWelcomeMessage}` | `{MlsGroupInfo}` |

### Engine: groups

| Method | Params | Result |
|---|---|---|
| `create_group` | `group_name` | `{MlsGroupInfo}` |
| `send_group_message` | `group_id`, `content`, `priority <MessagePriority>?`, `reply_to_msg?` | `[]` (message ids). Group sends carry the configured application id: the per-send id is a 1:1 option today |
| `forward_message_to_group` | `original_message_json`, `group_id`, `priority <MessagePriority>?` | `[]` (message ids) |
| `invite_to_group` | `group_id`, `invitee_user_id` | `null` |
| `remove_from_group` | `group_id`, `member_id` | `null` |
| `leave_group` | `group_id` | `null` |
| `list_groups` | | `[]` |
| `get_group_info` | `group_id` | `{MlsGroupInfo}` or `null` |
| `group_rich_readiness` | `group_id` | `{GroupRichReadiness}` |
| `group_relay_sync_state` | `group_id` | `<RelaySyncState>` |
| `request_group_relay_registration` | `group_id` | `!` |
| `set_member_role` | `group_id`, `user_id`, `role` | `null` |
| `get_member_role` | `group_id`, `user_id` | string |
| `get_group_roles` | `group_id` | object of user id to role |
| `rename_group` | `group_id`, `new_name` | `null` |

### Engine: blocking

| Method | Params | Result |
|---|---|---|
| `block_user` | `user_id` | `null` |
| `unblock_user` | `user_id` | `null` |
| `get_blocked_users` | | `[]` |
| `is_user_blocked` | `user_id` | `!` |

### Engine: transports and metrics

Read-only views of the instance. Everything here describes the whole server,
never one client's share of it.

| Method | Params | Result |
|---|---|---|
| `get_active_transports` | | `[]` |
| `get_transport_metrics` | `transport_type <TransportType>` | `{TransportMetrics}` or `null` |
| `should_escalate_to_wifi` | | `!` |
| `get_topology` | | `{NetworkTopology}` |
| `get_message_stats` | | `[{MessageStats}]` |
| `get_delivery_success_rate` | | `#` |
| `get_median_latency` | | `#` |
| `get_median_hops` | | `#` |
| `get_battery_level` | | `#` or `null` |
| `get_is_charging` | | `!` |
| `is_relay` | | `!` |
| `get_relay_priority` | | `<RelayPriority>` |
| `get_relay_config` | | `{RelayConfig}` |
| `get_dors_config` | | `{DorsConfig}` |
| `get_dedup_stats` | | `{DedupStats}` |
| `get_pending_ack_count` | | `#` |
| `get_retry_queue_size` | | `#` |
| `get_mesh_relay_stats` | | `{MeshRelayStats}` |
| `get_mesh_relay_tunables` | | `{MeshRelayTunables}` |
| `get_custody_stats` | | `{CustodyStats}` |

### Engine: instance-wide tuning

Every call here changes the engine for every client. They are on the wire
because an embedded application can call them, and because the socket is the
authorization boundary; an operator who does not want every application
retuning the relay denies the group by application id (see server-side
rules). None of them is per client and none is undone when the caller
disconnects.

| Method | Params | Result |
|---|---|---|
| `set_relay_priority` | `priority <RelayPriority>` | `null` |
| `update_relay_config` | `config {RelayConfig}` | `null` |
| `force_transport` | `transport_type <TransportType>` | `null` |
| `release_transport_lock` | | `null` |
| `update_dors_config` | `config {DorsConfig}` | `null` |
| `update_ack_config` | `config {AckConfig}` | `null` |
| `update_retry_config` | `config {RetryConfig}` | `null` |
| `update_dedup_config` | `config {DedupConfig}` | `null` |

### Services

The `MeshServices` object, one per server, under `services.`. Ownership is a
server-side rule stated below: a service is owned by the application id that
registered it.

| Method | Params | Result |
|---|---|---|
| `services.register_service` | `service_id`, `version`, `capabilities` (object) | `null` |
| `services.unregister_service` | `service_id` | `!` |
| `services.discover_services` | `service_id?` | string (query id) |
| `services.send_service_request` | `provider`, `service_id`, `method`, `body` | string (request id) |
| `services.respond_to_service_request` | `request_id`, `requester`, `service_id`, `status`, `body` | string (message id) |

### Documents

The `DataStore` object, one per server, under `data.`. Space scoping is a
server-side rule stated below.

| Method | Params | Result |
|---|---|---|
| `data.create_doc` | `space_id`, `doc_id` | `null` |
| `data.delete_doc` | `space_id`, `doc_id` | `null` |
| `data.remove_doc` | `space_id`, `doc_id` | `null` |
| `data.remove_space` | `space_id` | `null` |
| `data.set_interest` | `space_id`, `patterns []` | `null` |
| `data.list_docs` | `space_id` | `[]` |
| `data.list_spaces` | | `[]`, filtered to the spaces the client may open |
| `data.map_set` | `space_id`, `doc_id`, `collection`, `key`, `value_json` | `null` |
| `data.map_delete` | `space_id`, `doc_id`, `collection`, `key` | `null` |
| `data.map_get_json` | `space_id`, `doc_id`, `collection`, `key` | string or `null` |
| `data.list_push` | `space_id`, `doc_id`, `collection`, `value_json` | `null` |
| `data.list_delete` | `space_id`, `doc_id`, `collection`, `index #`, `count #` | `null` |
| `data.list_len` | `space_id`, `doc_id`, `collection` | `#` |
| `data.text_insert` | `space_id`, `doc_id`, `collection`, `position #`, `text` | `null` |
| `data.text_delete` | `space_id`, `doc_id`, `collection`, `position #`, `count #` | `null` |
| `data.text_value` | `space_id`, `doc_id`, `collection` | string |
| `data.counter_increment` | `space_id`, `doc_id`, `collection`, `amount #` | `null` |
| `data.counter_value` | `space_id`, `doc_id`, `collection` | `#` |
| `data.doc_json` | `space_id`, `doc_id` | string |
| `data.export_raw` | `space_id`, `doc_id` | `b64` |
| `data.flush` | `space_id`, `doc_id` | `null` |
| `data.flush_all` | | `null` |
| `data.doc_size` | `space_id`, `doc_id` | `#` |
| `data.attachment_hash` | `data b64` | string |
| `data.fetch_attachment` | `space_id`, `hash` | `null` |
| `data.provide_attachment` | `space_id`, `peer_id`, `hash`, `data b64` | `null` |
| `data.decline_attachment` | `space_id`, `peer_id`, `hash` | `null` |
| `data.fetch_attachment_from` | `space_id`, `peer_id`, `hash` | `null` |

### Instance-less

The definition's namespace functions that need no engine. They are the one
group a client may call before `hello`, because there is nothing to stamp and
nothing to route.

| Method | Params | Result |
|---|---|---|
| `derive_address` | `public_key b64` | string |
| `parse_invite` | `blob` | `{InviteInfo}` |
| `verify_identity_assertion` | `assertion b64` | string (the proved address) |

## Platform operations

Everything in the interface definition that is not in the method table is a
platform operation and is not on the wire. A request naming one is answered
with `-32601`, the same answer an unknown name gets, so a client cannot
discover the set by probing. The two tables together partition the
definition: every declaration is in exactly one, and the guard that reads
this chapter asserts it.

| Operation | Why the server owns it |
|---|---|
| `constructor`, `start`, `stop`, `pause`, `resume`, `process`, `receive_message`, `set_event_callback`, `poll_event`, `emit_test_event` | The lifecycle, the run loop and the drain (first invariant) |
| `initialize_mls`, `initialize_mls_with_file_stores`, `close_file_stores` | The identity's storage is attached once, by the process that holds the store key |
| `enable_telemetry`, `disable_telemetry`, `set_telemetry_enabled`, `flush_telemetry`, `flush_telemetry_blocking`, `telemetry_stats`, `end_telemetry_session`, `notify_app_state`, `telemetry_install_id` | The binding fills the platform fields and owns the session boundary ([C12](../bridges/README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform)); the server is the binding |
| `set_ble_transport_callback`, `set_wifi_direct_transport_callback`, `set_reticulum_transport_callback`, `set_nostr_transport_callback` | A callback interface has no JSON form, and each installs the driver of a carrier |
| `ble_peer_discovered`, `ble_peer_lost`, `ble_status_changed`, `ble_fragment_received`, `ble_get_next_fragment`, `ble_return_fragment`, `ble_get_peer_count`, `ble_set_peer_mtu`, `ble_clear_peer_mtu`, `ble_undersized_mtu_reports`, `ble_fragment_fallback_count`, `ble_recipient_not_among_peers_count`, `protocol_lock_diagnostics` | The Bluetooth LE driver's feed and drain, and the host's lock diagnostics |
| `internet_status_changed`, `internet_message_received`, `internet_get_next_message`, `internet_confirm_sent`, `internet_send_failed`, `internet_send_failed_with_reason`, `internet_peer_presence`, `internet_presence_watchlist`, `internet_relay_capabilities`, `internet_group_report_received`, `internet_address_declared`, `internet_address_declaration_refused` | The relay client's feed and drain, including the dedicated entry points the threat model's boundary 2 requires |
| `wifi_direct_status_changed`, `wifi_direct_message_received`, `wifi_direct_get_next_message`, `wifi_direct_peer_connected`, `wifi_direct_peer_disconnected` | The peer-stream driver's feed and drain |
| `reticulum_status_changed`, `reticulum_message_received`, `reticulum_get_next_message`, `reticulum_confirm_sent`, `reticulum_send_failed`, `reticulum_send_failed_with_reason`, `reticulum_address_declared`, `reticulum_address_declaration_refused`, `reticulum_gateway_capabilities`, `reticulum_peer_presence`, `reticulum_presence_watchlist` | The gateway client's feed and drain |
| `nostr_status_changed`, `nostr_message_received`, `nostr_message_received_at`, `nostr_get_next_message`, `nostr_confirm_sent`, `nostr_send_failed`, `nostr_send_failed_with_reason`, `nostr_get_public_key`, `nostr_get_subscription_filter`, `nostr_get_next_query`, `nostr_query_event_received`, `nostr_query_completed` | The Nostr client's feed and drain |
| `update_transport_metrics`, `remove_transport`, `set_battery_level`, `set_battery_state` | Facts about the host that only the host process knows |
| `identity_assertion`, `gateway_address_declaration` | Signatures a carrier driver presents to prove this device to a peer or a gateway; a client that could mint them could impersonate the server's carriers |
| `process_file_chunk`, `finalize_file` | The inbound chunk driver of a platform transport |
| `services.constructor`, `data.constructor`, `data.with_storage` | The server constructs the two objects once, over the engine it owns; `with_storage` takes a callback interface |
| `data.wipe_all` | Erases every client's documents and the identity's key documents at once. That is the operator's logout, taken at the server, never one application's call |
| `erase_custody` | Drops every frame this device holds for its neighbours, whichever client's traffic brought them, and resets the counters. The operator's erase, like `data.wipe_all`, never one application's call |
| `run_storage_conformance` | Takes a callback interface; a storage backend is verified by the host that supplies it |

## Event catalogue

### Routing

Every event the engine emits is relayed as a notification to the clients the
rules below select, in the order the engine emitted it. The rules are the
server's; they never change an event and they never reach the mesh.

1. **Stamped.** An event carrying an `app_id` field reaches the clients that
   declared that id, and no other. Three events are stamped today:
   `message_received`, `file_received` and `media_resend_required`. On the
   last two the field is optional, and the engine does emit `file_received`
   without it, for a transfer whose assembly had no metadata entry to read
   the id from. An event of a stamped tag whose `app_id` is absent is
   broadcast, not held, and logged, because the server has nothing to route
   or hold it by; dropping it would lose a message the engine counts as
   delivered.
2. **Correlated.** An event naming an identifier the server handed to one
   client as a method result (a message id, a file id, a query id, a request
   id) reaches that client, and no other. An event naming a service id
   reaches the clients whose application owns that service. The server keeps
   these tables in memory for the life of the process; an identifier issued
   before a restart, or one that came from a peer rather than from a client's
   call, is unknown, and an event naming an unknown identifier is broadcast.
   A `message_delivered` for a message a client sent therefore reaches that
   client and not its neighbours, and a `message_delivered` for a message the
   relay re-sent after a restart reaches everyone.

   An identifier stays live until the event that settles it:
   `message_delivered` or `message_failed` for a message id (a connection
   request's included), `media_sent` or `media_send_failed` for a file id,
   and `service_response_received` for a request id.
   `message_undeliverable` and `connection_request_undeliverable` settle
   nothing on a relay verdict: the engine keeps the message or the request,
   may repeat the event on every reachability probe, and later delivers it
   or gives up. A message a client sent to a recipient that is away
   therefore reports `message_undeliverable` to that client, as often as
   the engine says so, and its `message_delivered` reaches that client too.
   A settled identifier still routes to its client for the next 1024
   settlements, because the engine emits some events after the one that
   settles an id: a connection request it gives up on reports
   `message_failed` and then `connection_request_undeliverable`, and the
   second names the recipient.

   The engine emits synchronously, on the thread that is inside its call, so
   some events fire before the call that caused them has returned:
   `message_sent` is emitted inside the send when a transport takes the
   frame at once, and the server does not yet hold the id it will correlate
   by. An event emitted while the server is executing one client's call is
   therefore that client's, whatever it names, and the server records the
   identifiers it carries as that client's too. Without this rule the very
   first event of every send would be broadcast.
3. **Broadcast.** Everything else is engine-level and reaches every client.
   Group, presence, session and transport events are shared state of the one
   identity, so every application sees them.

The `Routing` column of the catalogue names the key: `app_id`, one of the
identifiers, or `all`.

### The catalogue

Every event the engine can emit, its fields, and how it is routed. The tag
is the `type` member; fields are the object's other members, using the legend
above with one difference: bytes in an event are an array of numbers, because
the event is relayed as the engine serialises it and the engine has always
written them that way. Sub-object shapes and the enum vocabularies follow the
table. The API reference documents about twenty of these with prose; this
table is read from the event definitions, held to them by the guard below,
and is the one a client is written against.

| Tag | Fields | Routing |
|---|---|---|
| `message_sent` | `message_id`, `sender`, `recipient`, `content`, `priority`, `requires_ack !`, `timestamp #`, `lamport_clock #`, `forward_info {ForwardInfo}?` | `message_id` |
| `message_received` | `message_id`, `sender`, `recipient`, `content`, `hop_count #`, `transport`, `timestamp #`, `lamport_clock #`, `reply_to_msg?`, `reply_context {ReplyContext}?`, `content_type`, `media_metadata {MediaMetadata}?`, `forward_info {ForwardInfo}?`, `encrypted !`, `app_id` | `app_id` |
| `message_delivered` | `message_id`, `latency_ms #`, `hop_count #`, `transport` | `message_id` |
| `message_failed` | `message_id`, `reason`, `retry_count #` | `message_id` |
| `message_decryption_failed` | `message_id`, `sender`, `code <DecryptionFailureCode>`, `reason` | `all` |
| `transport_switched` | `from?`, `to`, `reason` | `all` |
| `relay_promoted` | `connection_count #`, `battery_level #?` | `all` |
| `relay_demoted` | `reason` | `all` |
| `neighbor_discovered` | `peer_id`, `transport`, `rssi #?` | `all` |
| `neighbor_lost` | `peer_id` | `all` |
| `identity_ready` | `address` | `all` |
| `network_metrics` | `neighbor_count #`, `relay_count #`, `delivery_ratio #`, `avg_latency_ms #` | `all` |
| `file_progress` | `file_id`, `chunks_sent #`, `total_chunks #`, `percentage #` | `file_id` |
| `file_received` | `file_id`, `file_name`, `file_size #`, `sender`, `content_type`, `media_metadata {MediaMetadata}?`, `file_data` (base64), `timestamp #?`, `caption?`, `reply_to_msg?`, `reply_context {ReplyContext}?`, `forward_info {ForwardInfo}?`, `app_id?` | `app_id` |
| `file_receive_failed` | `file_id`, `file_name`, `sender`, `reason` | `all` |
| `media_sent` | `file_id`, `content_type`, `recipient` | `file_id` |
| `media_send_failed` | `file_id`, `recipient`, `reason` | `file_id` |
| `message_deferred` | `message_id`, `recipient`, `reason`, `retry_count #`, `next_retry_at #?` | `message_id` |
| `message_retrying` | `message_id`, `recipient`, `retry_count #`, `next_retry_at #` | `message_id` |
| `message_undeliverable` | `message_id`, `recipient`, `reason`, `file_id?` | `message_id` |
| `media_resend_required` | `file_id`, `recipient`, `file_name`, `file_size #`, `app_id?` | `app_id` |
| `ack_evicted` | `message_id`, `priority`, `reason` | `message_id` |
| `fragment_assembly_evicted` | `message_id`, `completion_percent #`, `reason` | `all` |
| `relay_demoted_battery` | `battery_level #`, `min_required #` | `all` |
| `secure_session_established` | `peer_id`, `group_id`, `is_session !`, `initiated_by_local !` | `all` |
| `secure_session_failed` | `peer_id`, `reason` | `all` |
| `convergence_diag` | `stage`, `peer_id`, `detail` | `all` |
| `welcome_send_attempted` | `peer_id`, `message_id`, `group_id`, `attempt #` | `all` |
| `welcome_send_succeeded` | `peer_id`, `message_id`, `group_id`, `attempt #` | `all` |
| `welcome_send_failed` | `peer_id`, `message_id`, `group_id`, `attempt #`, `reason_code <WelcomeReasonCode>`, `transport_error?`, `retryable !`, `next_retry_at #?` | `all` |
| `welcome_send_expired` | `peer_id`, `message_id`, `attempt #`, `reason_code <WelcomeReasonCode>` | `all` |
| `connection_request_received` | `sender`, `sender_name`, `timestamp #`, `key_package [#]?`, `initial_message?` | `all` |
| `connection_request_undeliverable` | `recipient`, `message_id`, `reason` | `message_id` |
| `connection_accepted` | `accepted_by`, `accepted_by_name`, `timestamp #`, `key_package [#]?` | `all` |
| `connection_rejected` | `rejected_by` | `all` |
| `connection_request_cancelled` | `cancelled_by` | `all` |
| `group_created` | `group_id`, `name` | `all` |
| `group_message_received` | `group_id`, `sender`, `content`, `timestamp` (a string), `message_id`, `reply_to_msg?`, `forward_info {ForwardInfo}?`, `media_metadata {MediaMetadata}?`, `content_type?` | `all` |
| `group_member_added` | `group_id`, `user_id`, `added_by`, `group_name?`, `authorized !?` | `all` |
| `group_member_removed` | `group_id`, `user_id`, `removed_by`, `authorized !?` | `all` |
| `group_info` | `group_id`, `name`, `created_by`, `created_at`, `members [{GroupInfoMember}]` | `all` |
| `user_groups` | `groups [{UserGroupSummary}]` | `all` |
| `group_error` | `reason`, `group_id?` | `all` |
| `group_relay_sync_changed` | `group_id`, `synced !`, `reason` | `all` |
| `username_resolved` | `username`, `claims [{UsernameClaim}]`, `rejected #`, `truncated #` | `all` |
| `group_message_sent` | `group_id`, `message_ids []`, `member_count #` | `all` |
| `group_message_partial_failure` | `group_id`, `failed_members []`, `succeeded_members []` | `all` |
| `group_message_delivery_report` | `group_id`, `message_id`, `delivered []`, `pushed []`, `missed_reissued []` | `all` |
| `group_rich_extras_dropped` | `group_id`, `unknown_members []` | `all` |
| `group_unauthorized_membership_change` | `group_id`, `committer`, `added []`, `removed []`, `reason`, `enforced !` | `all` |
| `group_epoch_fork_detected` | `group_id`, `local_epoch #?` | `all` |
| `group_epoch_fork_resolved` | `group_id`, `resolved_epoch #`, `failed_members []` | `all` |
| `group_role_changed` | `group_id`, `user_id`, `new_role`, `changed_by` | `all` |
| `group_renamed` | `group_id`, `new_name`, `old_name?`, `renamed_by` | `all` |
| `service_discovered` | `query_id`, `service_id`, `version`, `provider_peer_id`, `capabilities` (object), `hop_count #` | `query_id` |
| `service_request_received` | `request_id`, `service_id`, `method`, `body`, `sender` | `service_id` |
| `service_response_received` | `request_id`, `service_id`, `status`, `body`, `provider_peer_id` | `request_id` |
| `presence_updated` | `peer_id`, `status <PresenceStatus>`, `timestamp #`, `last_seen_ms #?`, `source <PresenceSource>` | `all` |
| `typing_indicator_received` | `sender`, `conversation_id`, `is_typing !`, `timestamp #` | `all` |
| `read_receipt_received` | `sender`, `message_ids []`, `timestamp #` | `all` |
| `dors_score_updated` | `scores` (array of `[transport, score #]` pairs) | `all` |
| `dors_transport_selected` | `from?`, `transport`, `reason_code <DorsReasonCode>`, `score #` | `all` |
| `dors_transport_switched` | `from?`, `to`, `reason_code <DorsReasonCode>`, `reason_detail?` | `all` |
| `dors_escalation_triggered` | `phase <DorsEscalationPhase>`, `from`, `to`, `reason_code <DorsEscalationReasonCode>`, `reason_detail?` | `all` |
| `security_warning` | `peer_id`, `reason_code <SecurityWarningCode>`, `reason` | `all` |
| `message_relayed` | `message_id`, `sender`, `recipient`, `hop_count #`, `remaining_ttl #` | `all` |
| `user_blocked` | `user_id` | `all` |
| `user_unblocked` | `user_id` | `all` |
| `data_changed` | `space_id`, `doc_id`, `delta_bytes #` | `all` |
| `data_doc_removed` | `space_id`, `doc_id`, `by <DocRemovedBy>` | `all` |
| `data_doc_size_warning` | `space_id`, `doc_id`, `compacted_bytes #`, `cap_bytes #` | `all` |
| `data_attachment_requested` | `space_id`, `peer_id`, `hash` | `all` |
| `data_attachment_received` | `space_id`, `peer_id`, `hash`, `data` (base64) | `all` |
| `data_attachment_unavailable` | `space_id`, `peer_id`, `hash`, `reason` | `all` |
| `data_doc_unsyncable` | `space_id`, `doc_id`, `bytes #`, `reason` | `all` |

Two rows deserve a note. `message_decryption_failed` stands in for a message
that could not be read, and the event does not carry the frame's application
id even though the frame did, so it is broadcast; when the engine adds the
id (additive under C3) it becomes stamped. `message_relayed` reports a frame
this device forwarded for others and names another identity's message id,
which is why it is broadcast rather than correlated.

Document events (`data_*`) are broadcast, and a client MUST apply its own
space filter to them: the server's space allow-list gates calls, and the
events carry the space id for the client to match, so a client that may not
open a space still learns that it changed. Filtering them at the server is a
server-side rule an implementation MAY add, and the reference server does.

### Shapes

| Object | Members |
|---|---|
| `ForwardInfo` | `original_sender`, `original_message_id`, `original_timestamp #`, `forward_count #` |
| `ReplyContext` | `sender`, `text`, `timestamp #?`, `reply_media_label?`, `reply_content_type?` |
| `MediaMetadata` | `mime_type`, `file_name`, `file_size #`, `duration_ms #?`, `width #?`, `height #?`, `thumbnail_base64?`, `media_id?`, `download_url?`, `thumbnail_url?`, `encryption_key?`, `iv?`, `ciphertext_hash?`, `sticker_provider?`, `sticker_remote_id?`, `sticker_kind?` |
| `GroupInfoMember` | `user_id`, `role`, `joined_at` |
| `UserGroupSummary` | `group_id`, `name`, `created_at` |
| `UsernameClaim` | `address`, `public_key`, `issued_at_ms #` |

### Vocabularies

| Enum | Values |
|---|---|
| `PresenceStatus` | `online`, `away`, `offline` |
| `PresenceSource` | `internet`, `reticulum`, `peer` |
| `DocRemovedBy` | `local`, `peer` |
| `DecryptionFailureCode` | `INVALID_PAYLOAD`, `NOT_INITIALIZED`, `INVALID_CIPHERTEXT`, `IDENTITY_MISMATCH`, `CRYPTO_FAILURE`, `PENDING_QUEUE_DROPPED`, `UNKNOWN` |
| `WelcomeReasonCode` | `TRANSPORT_UNAVAILABLE`, `PEER_UNREACHABLE`, `PEER_DISCONNECTED`, `TIMEOUT`, `INTERNAL_ERROR`, `RETRY_EXHAUSTED` |
| `DorsReasonCode` | `INITIAL_SELECTION`, `PRIMARY_SELECTED`, `PRIMARY_SUCCESS`, `FALLBACK_SUCCESS`, `ESCALATION_APPLIED`, `CURRENT_UNAVAILABLE` |
| `DorsEscalationPhase` | `TRIGGERED`, `APPLIED` |
| `DorsEscalationReasonCode` | `FALLBACK_SUCCESS`, `RETRY_THRESHOLD`, `POOR_SIGNAL`, `CONGESTION`, `LOW_TTL`, `LOW_SUCCESS_RATE` |
| `SecurityWarningCode` | `SENDER_ADDRESS_MISMATCH`, `TRANSPORT_IDENTITY_MISMATCH`, `CONTROL_SIGNATURE_INVALID`, `UNSIGNED_CONTROL_REJECTED`, `MEDIA_SENDER_GROUP_MISMATCH`, `PLAINTEXT_SEND`, `PLAINTEXT_RECEIVE_REJECTED`, `SESSION_SENDER_GROUP_MISMATCH`, `SESSION_REKEY_TRIGGERED`, `NOSTR_KEY_PACKAGE_SLOT_EXHAUSTED`, `PUSH_KEY_PACKAGE_POOL_EXHAUSTED`, `RELAY_ADDRESS_BINDING_MISMATCH`, `RELAY_ADDRESS_DECLARATION_REFUSED`, `GROUP_LEAF_IDENTITY_UNPROVEN`, `STALE_CONTROL_FRAME`, `GATEWAY_ADDRESS_BINDING_MISMATCH`, `GATEWAY_ADDRESS_DECLARATION_REFUSED`, `KEY_PACKAGE_OUTSIDE_VALIDITY_WINDOW` |

The transport name is spelled two ways by the engine, and a client accepts
both. `message_received`, `message_delivered`, the DORS events and a
`transport_switched` the engine emits carry the transport's label: `ble`,
`wifiDirect`, `internet`, `reticulum`, `nostr`, or `delayed` on a
`message_received` for a message that was held and decrypted later.
`neighbor_discovered`, and a `transport_switched` the FFI layer emits when a
carrier's status changes, carry the casing the platform bridge passes as a
string: `BLE`, `WiFiDirect`, `Internet`, `Reticulum`, `Nostr`, and on that
`transport_switched` the literal `None` as `to` when a carrier disconnects
and nothing takes over. Neither spelling is the interface definition's enum
(`Ble`), and `wifi_direct` never occurs. The `priority` string is spelled
two ways by the engine and a client accepts both: lowercase on `message_sent`
(`low`, `medium`, `high`, `critical`) and capitalised on `ack_evicted` (`Low`,
`Medium`, `High`, `Critical`).

These are the spellings the events carry. A method parameter of an enum type
uses the interface definition's spelling, so a client sends `"Online"` to
`send_presence_update` and reads `"online"` back on `presence_updated`.

The vocabularies above are the engine's, pinned against the TypeScript
declarations by the guards [T2](../bridges/typescript.md#t2-event-types-are-pinned-from-the-rust-side)
names; this chapter restates them so a client author has one place to read,
and the guard for this chapter (below) holds the tag column to the engine.

## Replay

The interface that drives a radio has a rule for events that fire before
anyone is listening ([C10](../bridges/README.md#c10-lifecycle-rules-for-event-emission)),
and a server has the same problem one level up: an application's client may
not be connected when its message arrives. The engine has already
acknowledged that message, dedup-marked its id and dropped its queued copy
by the time `message_received` is emitted, so the sender will not resend and
nothing in the engine will restate the event. A server that dropped it
would lose a message that the mesh counts as delivered.

So a server holds **stamped inbound events** for an application id with no
connected client: two of C10's three tags, `message_received` and
`file_received`, plus `media_resend_required` because it is stamped too.
C10's third tag, `message_decryption_failed`, is excluded until it carries an
`app_id`, because there is no id to hold it under. Each held event is kept
whole, in arrival order, in a per-application-id FIFO capped at 256 entries
with the oldest dropped past the cap. The cap is the same real capacity the
mobile bridges use, and it is a capacity rather than a backstop: past it,
messages are lost, and a server SHOULD log each drop. When a client next
declares that application id and the server accepts the `hello`, the server
delivers the held events to it, in order, after the `hello` result and
before any newer event for that id. Delivery is **at least once**: a client
that disconnected mid-delivery may see an event twice, and message ids are
the idempotency key. An event of a stamped tag that arrives with no `app_id`
is not held (see routing), because there is nothing to hold it under.

Nothing else is held. A broadcast event describes state the next event of
its kind restates, and a correlated event names an identifier whose client
was connected when it was issued; a server MAY hold correlated events for a
client that disconnected and reconnected under the same application id, and
MUST NOT hold them past the process's life. When `message_decryption_failed`
becomes stamped it joins the held set.

The hold is per application id and shared by every client of that id: a
second process of the same application that connects while the first is
connected receives nothing from the hold, because the first already did.

## Errors

A failed call is a JSON-RPC error object. The `code` is an integer because
the framing requires one, and the client switches on the variant name, which
is what the engine's own bindings switch on:

```json
{"jsonrpc":"2.0","id":2,"error":{"code":-32004,"message":"No key package available for recipient: off1...","data":{"variant":"NoKeyPackage"}}}
```

- **`error.data.variant`** is the `ProtocolError` variant name from the
  interface definition, unchanged. The taxonomy is the engine's, all
  twenty-five variants of it, and the distinctions the engine draws survive:
  `NotStarted` is an engine that is not running, `InvalidState` is an
  operation that is wrong for the engine's current state or for a
  resource's, and `InvalidConfiguration` is a value the engine refused,
  where retrying with the same value cannot help.
- **`error.message`** is the variant's display string, with the detail the
  engine attached.
- **`error.code`** is `-32000 - n`, where `n` is the variant's position in
  the definition's error enum, zero-based. The enum is append-only by
  contract ([C2](../bridges/README.md#c2-the-error-enum-is-append-only)),
  which is the same rule that keeps the generated bindings decoding it, so a
  position never changes and a variant appended later takes the next
  number without anyone assigning it. The position is zero-based, so
  `NotStarted` is `-32000`; the discriminant the generated bindings use for
  the same variant is one-based (`NotStarted` is `1`), and the two are not
  meant to agree, so nobody should "correct" one to the other. The
  server-defined range the JSON-RPC specification reserves runs to
  `-32099`; the first fifty numbers are this taxonomy's, and nothing else is
  ever assigned in them.
- **The standard codes** keep their standard meaning: `-32700` parse error,
  `-32600` invalid request (including a batch), `-32601` method not found
  (including every platform operation), `-32602` invalid params (a missing
  required parameter, a positional array, a value of the wrong JSON type),
  `-32603` an internal failure of the server itself, never of the engine.
- **Session refusals reuse the taxonomy.** A method before `hello`, or a
  second `hello`, is `InvalidState`; a bad application id is
  `InvalidArgument`; a wrong token, a service another application owns, a
  space outside the client's allow-list, and a method group the operator
  denied are all `PermissionDenied`. No variant exists only on this wire.

| Variant | Code | The engine's meaning |
|---|---|---|
| `NotStarted` | `-32000` | The engine is not running. Retry after the server starts it |
| `AlreadyStarted` | `-32001` | Not reachable from a client; `start` is the server's |
| `InvalidConfiguration` | `-32002` | A value was refused. Retrying with the same value cannot help |
| `SendFailed` | `-32003` | The transport refused or lost the send |
| `NoKeyPackage` | `-32004` | No key package for the recipient yet |
| `SessionNotReady` | `-32005` | Establishment in progress; the message names the state |
| `EncryptFailed` | `-32006` | Outbound encryption failed |
| `InvalidState` | `-32007` | Wrong for the current state of the engine or of the named resource |
| `MlsNotInitialized` | `-32008` | The server has not attached storage |
| `MlsError` | `-32009` | An MLS operation failed |
| `UserBlocked` | `-32010` | The target is blocked |
| `MediaTransferLimit` | `-32011` | Too many transfers to one recipient; retry after one completes |
| `LockPoisoned` | `-32012` | A thread panicked inside the engine |
| `Other` | `-32013` | Unclassified |
| `TransportError` | `-32014` | A transport failure outside a send |
| `SerializationError` | `-32015` | A payload did not serialise or parse |
| `ServiceError` | `-32016` | A mesh service registry or request failure |
| `GroupNotFound` | `-32017` | The group does not exist locally |
| `PermissionDenied` | `-32018` | A role the caller lacks, or a server-side rule |
| `InvalidArgument` | `-32019` | A caller-supplied value failed validation |
| `DataDisabled` | `-32020` | The data layer is off in configuration |
| `DataStorageUnavailable` | `-32021` | The data layer has no storage yet |
| `DocTooLarge` | `-32022` | The document is over its cap |
| `DataCorrupted` | `-32023` | A stored document could not be read |
| `TelemetryConfigInvalid` | `-32024` | Not reachable from a client; telemetry is the server's |

## Server-side rules

Four rules separate the applications behind one server. Each is applied by
the server before a call reaches the engine, each refuses with
`PermissionDenied`, and none has any representation on the mesh: a peer
sees one identity registering services and syncing spaces, exactly as it
would from one application. They separate applications from each other's
mistakes, not from a hostile process inside the socket boundary (fourth
invariant).

**Unlisted ids.** With no rule configured at all, a server accepts any
well-formed application id at `hello`. Once any space allow-list or method
deny is configured, the set of ids those entries name is the set of
applications the operator knows about, and a `hello` declaring an id with
no entry is refused with `PermissionDenied`. Service ownership does not
count: it is a runtime shadow the clients' own registrations build, not
something the operator configured, so a first registration never flips an
open server to default-deny. Without this rule the two configured rules are
void: an application whose id is denied a method, or scoped to a space,
reconnects under an id no rule names and is denied nothing and scoped to
nothing. An operator who wants an open server configures no rule; an
operator who configures one has listed every application.

**Service ownership.** `services.register_service` records the calling
application id as the owner of that service id. `services.unregister_service`
and `services.respond_to_service_request` for a service another application
owns are refused; `service_request_received` for it is routed to the owner's
clients. The engine has no owner on a registration and no way to enumerate
them, so the server's registry is a shadow of the engine's, rebuilt from
the clients' calls, and empty after a restart until they register again. An
application that registers a service id another application already holds is
refused, which is the collision the registry exists to catch: without it the
second registration silently replaces the first in the engine and the first
application's requests start arriving at the second.

**Space scoping.** The operator's configuration MAY map each application id
to a list of glob patterns over space ids. Every `data.*` call naming a space
outside the caller's patterns is refused, `data.list_spaces` is filtered to
them, and a document event for a space outside them is not delivered. An
application with no entry may open every space. The failure this prevents is
two applications choosing the same space name by accident and merging each
other's documents, which the engine cannot notice because to it there is one
member.

**Method groups.** The operator's configuration MAY deny a named group of
methods to an application id: `sign_data`, the manual MLS group, the
instance-wide tuning group, or any single method. A denied method is refused
with `PermissionDenied`, not `-32601`, because the method exists and the
refusal is a policy the client should be able to read. Nothing is denied by
default.

## What a guard pins

The method table and the platform table together name every declaration in
the interface definition exactly once, and the catalogue names every event
tag the engine can emit. Both claims rot silently: a method added to the
definition and to no table is a method a client cannot find, or worse one the
reference server exposes without this chapter saying so; an event added to
the engine and not here is one no client is written for.

A Rust guard in the FFI crate reads this chapter, the interface definition,
the engine's event definitions and the reference server's dispatch table, and
asserts:

1. the set of names in the method table (with `services.` and `data.`
   stripped) plus the set in the platform table equals the set of
   declarations in the definition, with no name in both;
2. the reference server's dispatch table exposes exactly the method table's
   names and no platform name;
3. every variant of the engine's event enum appears as a tag in the
   catalogue, and every tag in the catalogue is a variant.

The tables are written to be read by that guard as well as by a person: a
row's first cell is the backticked name, the method table runs from the
heading `Method table` to the heading `Platform operations`, the platform
table from there to `Event catalogue`, and the catalogue's rows are the ones
whose first cell is a backticked tag between `The catalogue` and `Shapes`.
Rows in the platform table list several names in one cell; the guard reads
every backticked token in the first cell. A change to those headings is a
change to the guard.

## What this chapter does not cover

- **The reference server's own configuration**: socket paths, the token
  file, the allow-lists, the store key. Those are its guide's, and a second
  server may configure them however it likes.
- **Transport drivers.** A server that wants Bluetooth LE or a peer stream
  runs the driver in its own process, against the platform operations. This
  chapter says only that a client cannot.
- **Authentication of anything but the socket.** There is no account, no
  per-application credential, and no way for one application to prove to the
  server that it is not another. An operator who needs that runs one server
  per identity, which is one server per set of mutually trusting
  applications.
- **A network-facing API.** Loopback only, by construction, in this version.
