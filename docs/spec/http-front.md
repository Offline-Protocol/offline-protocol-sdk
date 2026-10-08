# The HTTP front

## What this chapter is for

[The local API](local-api.md) lets a local application drive the engine over
JSON-RPC on a WebSocket. That is the right shape for an application written
against this SDK, and the wrong one for an application that should not know
the SDK exists: a program that already speaks HTTP and wants to call a
service on another device the way it calls any other HTTP server.

The HTTP front is that second shape. It is an HTTP server on the same host
as the engine. An application sends an ordinary HTTP request to a hostname
that names a device and a service; the front carries the request to that
device inside an end-to-end encrypted message, the front there calls the
HTTP endpoint the providing application registered, and the answer comes
back the same way as an ordinary HTTP response. Nothing in either
application imports the SDK.

This chapter fixes the hostname scheme, the envelope two fronts exchange,
the limits, what each side does with a request and a response, the
registration and browse endpoints, and the error tokens. It does not cover
how a host routes a hostname to the front (a resolver entry, a wildcard DNS
rule, a manual `Host` header), which is the host's business, nor tunnelling
arbitrary TCP, which a message-oriented protocol does not offer.

## Invariants

These outrank every mechanism below. A front that keeps the mechanisms and
breaks an invariant is wrong.

1. **A body leaves the host only inside a sealed message.** Requests and
   responses travel as the content of an ordinary direct message, which the
   engine encrypts end to end with MLS. A front MUST NOT carry a request or
   response body on the service request path, whose bodies are signed but
   not encrypted ([R9](../security/threat-model.md#r9-service-discovery-and-service-bodies-are-signed-not-encrypted)).
   The failure this prevents: every body readable by every device that
   forwards it, on a carrier with no hop encryption at all.
2. **A front acts only on a message the engine decrypted.** It processes an
   envelope only from a `message_received` whose `encrypted` is `true`. The
   engine then refuses a wire sender that differs from the MLS-authenticated
   one, so `sender` names the device that sealed the message, and that is
   the identity a provider is told. A plaintext message carrying an envelope
   is dropped. The application id stamped on the message selects the front;
   it is cleartext and authorizes nothing
   ([R17](../security/threat-model.md#r17-the-application-id-on-every-frame-is-cleartext-and-unsigned)).
3. **A device label comes from the address itself or from the operator.** A
   label resolves to an address only as the literal address or through the
   operator's alias file. A front MUST NOT learn an alias from a LAN record,
   a discovery response or anything else a peer says. The failure this
   prevents: any host on the segment binding a friendly name to its own
   address, which is authentic, so the binding is the forgery and nothing
   downstream can notice it.
4. **A field that does not fit is refused with a status, never truncated.**
   A truncated path names another resource and a truncated body is another
   document.
5. **Discovery is a claim.** Browse results report what devices answered a
   signed discovery query, or what an unsigned LAN record said, and mark
   which. They never become aliases (invariant 3).

## The hostname

```
<service>.<device>.<domain>
```

`domain` is configurable and defaults to `offline.protocol.internal`.
Hostnames are compared in lower case, after removing any port and any
trailing dot.

- **`service`** is a DNS label: `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`. A
  provider registers under exactly this name, so a service a front can
  publish is always one a requester can name.
- **`device`** is either an address, `off1` followed by 40 characters of the
  bech32 alphabet (an address is 44 characters of `[a-z0-9]`, so it is a DNS
  label as it stands), or an alias the operator's alias file defines.
- An alias obeys the service grammar and MUST NOT itself parse as an
  address. A file that defines one is refused at load: an alias that looks
  like an address would shadow a real one.

A request whose host is the domain itself, or is not under the domain at
all (a request to `127.0.0.1:8080`), is for the front's own endpoints below.
A host under the domain with other than exactly two labels before it is
refused with `bad_host`.

The alias file is JSON:

```json
{"aliases": {"bob": "off1qx7jj4u8w32ptzysnkadwjzmz9w2nukfmc3ts2ap"}}
```

## The envelope

Two fronts exchange JSON objects as the content of a direct message. Every
message a front sends carries the application id
`offline-protocol-http-front`, so the receiving host's local API routes it to
the front and holds it while the front is away. A host runs one front: the
local API delivers a message stamped with that id to every client that
declared it, so a second front would call the callback again for every
request.

```json
{"op":"req","v":1,"id":"1f0e…","svc":"timeofday","m":"POST","p":"/now?tz=utc",
 "h":{"content-type":"application/json"},"b":"eyJ6b25lIjoidXRjIn0=","dl":1760000000}

{"op":"resp","v":1,"id":"1f0e…","s":200,"h":{"content-type":"application/json"},"b":"…"}

{"op":"resp","v":1,"id":"1f0e…","s":404,"e":"unknown_service"}
```

| Key | In | Meaning |
|---|---|---|
| `op` | both | `req` or `resp` |
| `v` | both | Envelope version, `1` |
| `id` | both | A random identifier the requester chose; the response repeats it |
| `svc` | req | The service label |
| `m` | req | The HTTP method, an HTTP token, upper case |
| `p` | req | The path and query, starting with `/`, with no `.` or `..` segment |
| `h` | both | Carried headers, names in lower case |
| `b` | both | The body as standard base64; absent or empty for none |
| `dl` | req | The deadline, Unix seconds |
| `s` | resp | The HTTP status, 200 to 599 |
| `e` | resp | An error token (below), present only when the front failed |

An envelope with an unknown `op` or a version other than `1` is not
processed. A request with an unknown version is answered with
`unsupported_version` when its `id` can be read.

An envelope is not processed either when `m` or a header name is not an
HTTP token, a header value or `p` holds a control character other than
horizontal tab, `p` holds a `.` or `..` segment in its path part, raw or
percent-encoded, or `s` is an interim 1xx status. An HTTP client refuses the
first two at write time, which would leave the far side unable to answer,
and a requester that relayed a CR or LF would split its own response. An
HTTP client normalizes dot segments away, `/api/%2e%2e/admin` included, so
joined to the callback URL such a path reaches one outside the path the
provider registered. A 1xx written as the answer leaves the client waiting
for the real one. A string with no UTF-8 form (a JSON `\udcff` escape) is
not processed either. On the requester, a carried header value that is not
UTF-8, which HTTP allows, is `bad_request`: the far side could not decode it.
A token header that is not UTF-8 is `unauthorized`, like any wrong token. On
the provider, a callback that answers such a header value is
`callback_failed`.

### Limits

| Limit | Value | Refused with |
|---|---|---|
| Body, raw bytes, either direction | 128 KiB | `body_too_large` (413) or `response_too_large` (502) |
| Carried headers, names plus values | 8 KiB | `bad_request` (400) |
| Path and query | 2 KiB | `bad_request` (400) |
| Deadline, default | 30 s from the request | |
| Deadline, maximum | 300 s from the request | clamped |
| Provider clock slack | 60 s | a request older than this past its deadline is dropped |
| Callbacks running at once, per provider | 32 | `callback_failed` (502), at once |

The body limit keeps a base64 body and its envelope under the engine's
256 KiB content limit with room to spare. It does not fit every carrier: a sealed
envelope over about 71 KB does not cross Bluetooth LE at the 185-byte MTU
floor, nor one over 64 KiB a Nostr relay. The engine retries such a message
as it retries any other, so a requester whose only route is one of those
sees `deadline_exceeded`, not `send_failed`. A requester may ask for a shorter
or longer deadline with `X-Offline-Protocol-Timeout: <seconds>`, clamped to
1 and 300.

### Carried headers

A header crosses only if it is on this list, in either direction:
`accept`, `accept-language`, `authorization`, `cache-control`,
`content-language`, `content-type`, `etag`, `if-match`, `if-none-match`,
`last-modified`, `location`, and any name starting `x-` except those
starting `x-offline-protocol-` or `x-forwarded-`, and `x-real-ip`,
`x-original-url` and `x-rewrite-url`. Everything else, and every hop-by-hop
header, is dropped. `host` and `content-length` are never carried: the
first is the requester's routing and the second is recomputed. The
forwarding names are dropped because a reverse proxy in front of a callback
reads them as the client's address or the original target, and the front
calls the callback from loopback, which such a proxy trusts most: a
requester that could set them would choose the source the callback sees, or
a path outside the registered one.

## The requester

1. Parse the host. Resolve the device (invariant 3). An unknown alias is
   `unknown_device` (404).
2. Read the body, refusing one over the limit, and build a `req` envelope
   with a fresh `id` and the deadline.
3. Send it with `send_message` to the device. The engine forms the session
   itself if none exists, and holds the message until it does.
4. Wait for a `resp` with the same `id` from that device, until the
   deadline (`deadline_exceeded`, 504). A `message_failed` naming the
   message ends the wait with `send_failed` (502), and a
   `message_undeliverable` with `recipient_unreachable` (502).
5. Answer with the response's status, carried headers and body. A response
   with `e` is answered with its status and the token in
   `X-Offline-Protocol-Error`.

A `resp` whose `id` is not waiting, or that comes from a device other than
the one asked, is dropped. A response that arrives after the deadline is
dropped.

A request to this device's own address is served locally, without a
message, by the same steps the provider takes.

## The provider

On a `message_received` stamped with the front's application id and with
`encrypted` true, whose content parses as a `req`:

1. Drop it when its deadline is more than the clock slack in the past: the
   requester has stopped waiting.
2. Look up the service. One that is not registered is `unknown_service`
   (404).
3. Call the registered callback: the method, the callback URL joined with
   the path, the carried headers, the body, and three headers of the
   front's own:
   `X-Offline-Protocol-Sender` (the authenticated sender's address),
   `X-Offline-Protocol-Request-Id` and `X-Offline-Protocol-Service`. The
   callback's timeout is the time left before the deadline, at least 5 s
   and at most the maximum deadline. With 32 callbacks already running the
   request is answered `callback_failed` at once rather than queued: any
   device with a session can send requests, each can hold a connection to
   the callback for the whole maximum deadline, and a queue would hold every
   later request, from every device, behind whoever filled it. The limit
   is per provider, not per sender: it bounds the load on the callback,
   and one device can still hold every slot.
4. Send a `resp` with the callback's status, carried headers and body to the
   sender. A callback that fails or times out is `callback_failed` (502),
   and a body over the limit is `response_too_large` (502).

**The provider authorizes; the front does not.** Any device whose message
the engine decrypts can call any registered service. A provider that serves
only some devices reads `X-Offline-Protocol-Sender` and refuses the rest.

## The front's own endpoints

Requests whose host is not under the domain, or is the domain itself:

| Method and path | Does |
|---|---|
| `PUT /services/<service>` | Registers `{"callback": "http://…", "version": "", "capabilities": {}}`. Idempotent. `409` when another local application owns the name |
| `DELETE /services/<service>` | Unregisters |
| `GET /services` | Lists this front's registrations |
| `GET /browse/<service>` | Server-sent events: who offers the service |
| `GET /health` | `{"name", "version", "connected", "local_address"}` |

A registration goes to the engine's service registry as well, so signed
discovery finds it, and the front keeps its registrations in a file so a
restart registers them again. The callback is an `http` or `https` URL
with no query or fragment, since the request's path is appended to it; the
front calls it as given.

**Browse** streams `added` and `gone` events, each with `device`, `alias`
(when the operator's file maps one to the device), `service`, `version`,
`capabilities`, `hops` and `source` (`mesh` or `lan`), and a comment line
every 15 s to keep idle connections open. While any browser is open for a
service, the front issues one discovery query for it every 60 s, never one
per browser or per request: discovery is a broadcast to up to 20
neighbours gossiped up to 10 hops. A provider that answers no query for two
rounds is reported `gone`.

### Authorization

The front's HTTP side admits any process that can reach its socket. Bound to
loopback, which is the default, that is the host's own processes; bound to
any other address, the front requires a token: a fresh 32-byte value at every
launch, written to a file with mode `0600` in the same call that creates it,
presented in `X-Offline-Protocol-Token`. A wrong or missing token is
`unauthorized` (401). A request carrying an `Origin` header is
`unauthorized` (401) on any binding: a browser sends one on every
cross-origin `fetch` or `XMLHttpRequest` and on every method other than
`GET` and `HEAD`, and no other client does, so this is what stops a web page
the user opens from sending a body as this device or registering a callback
of its choosing through a wildcard route to the domain or DNS rebinding.
Without a token, the front's own endpoints answer only a host that is the
domain, `localhost` or a loopback address, and refuse any other with
`bad_host` (400): a rebinding page reaches the front under its own name, and
a same-origin `GET` carries no `Origin`.

That leaves one route open without a token. A `GET` a page makes by loading
a resource (an image, a script, a frame, a navigation) carries no `Origin`,
and `Sec-Fetch-*` is not sent to a plain-`http` name that is not loopback,
so with a wildcard route to the domain on a host where a browser runs, any
page can make a blind `GET` to `<service>.<device>.<domain>` as this device.
It cannot read the answer or set a header, so a front with a token refuses
it. A host where a browser runs and the domain resolves to the front should
run the front with a token. Telling one local application from another is the
host's to provide (a per-application network namespace, or a header the
host's own router adds), and is recorded as
[R23](../security/threat-model.md#r23-the-http-front-trusts-every-process-that-can-reach-it).

## Error tokens

| Token | Status | Raised by | Means |
|---|---|---|---|
| `bad_host` | 400 | requester | The host is under the domain but not `<service>.<device>`, or, without a token, names neither the domain nor loopback |
| `bad_request` | 400 | requester | Headers or path over their limit, a header value that is not UTF-8, a path with a dot segment, or a timeout that is not a number |
| `unknown_device` | 404 | requester | The label is neither an address nor an alias |
| `body_too_large` | 413 | requester | The request body is over the limit |
| `deadline_exceeded` | 504 | requester | No response before the deadline |
| `recipient_unreachable` | 502 | requester | The engine reported the device unreachable |
| `send_failed` | 502 | requester | The engine gave up on the request message |
| `not_connected` | 503 | either | The front has no connection to the local API, or is stopping |
| `unauthorized` | 401 | either | The token is missing or wrong, or the request carries `Origin` |
| `unknown_service` | 404 | provider | Nothing registered under that name |
| `callback_failed` | 502 | provider | The callback refused the connection, failed, timed out or answered a 1xx, or 32 callbacks were already running |
| `response_too_large` | 502 | provider | The callback's body is over the limit |
| `unsupported_version` | 400 | provider | The envelope version is not 1 |

## What a guard pins

The limits, the application id, the domain default, the carried-header list,
the dropped forwarding names and the error tokens are literals in the reference front
(`offline_protocol_sdk.http_front`), pinned as literals in its tests rather
than read from the module, so an edit to either side is a failing test
([C5](../bridges/README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language)).
