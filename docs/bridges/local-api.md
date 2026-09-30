# Local API contract

Covers a server that fronts one engine for several local clients over the
[local API](../spec/local-api.md), and the reference server in the Python
package.

Read [the shared contract](README.md) first. A server is a binding one level
up: it is written against a generated binding (the reference one against
Python), so everything that binding owes, the server owes through it, and
this document covers what is specific to sitting between the binding and a
socket.

## Which shared rules apply verbatim

| Rule | Why it reaches the server |
|---|---|
| [C1](README.md#c1-regenerate-every-binding-together) | The server calls a generated binding. A partial regeneration fails at the server's first call, at runtime, exactly as it does in an application |
| [C2](README.md#c2-the-error-enum-is-append-only) | The JSON-RPC error code is the variant's position in the enum. Append-only is what makes that number stable; an inserted variant would renumber every client's error handling as well as breaking the bindings |
| [C3](README.md#c3-events-cross-as-opaque-json) | An event notification's `params` is the engine's JSON, unchanged. The server never parses an event for anything but routing (`type`, `app_id`, the correlation identifiers), and never re-encodes one |
| [C6](README.md#c6-config-parsers-must-not-default-to-literals) | A dictionary parameter arrives partial. The server passes the fields the client sent and lets the definition's defaults fill the rest; a literal fallback in the server would reset every field a client did not mention |
| [C10](README.md#c10-lifecycle-rules-for-event-emission) | The per-application-id hold of stamped inbound events is C10's held inbound buffer, one level up: the same 256-entry capacity and the same reason (the engine has already acknowledged the message), over two of C10's three tags (`message_received`, `file_received`) plus `media_resend_required` because it is stamped; `message_decryption_failed` is excluded until it carries an `app_id` |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | The server is the binding that fills the platform fields and owns the session boundary. Telemetry is therefore a server-owned operation, and no client can enable, disable or flush it |

## L1. The method and event tables are pinned by a Rust guard

The chapter carries three tables that name things the compiler never sees
together: the methods a client may call, the platform operations it may not,
and the events it may receive. Each rots silently. A method added to the
interface definition and to neither table is unreachable from every client
and unmentioned in the contract; an event added to the engine and not to the
catalogue is one no client is written for; a method the reference server
exposes that the chapter lists as platform-only is a hole in the boundary
the chapter's fifth invariant draws.

A Rust guard in the FFI crate reads the chapter, the interface definition,
the engine's event enum and the reference server's dispatch table, and
asserts that the exposed set plus the platform set is exactly the set of
declarations, that the dispatch table exposes exactly the exposed set, and
that the catalogue's tags are exactly the enum's variants. It follows the
skip-if-tree-absent idiom the other document-reading guards use, so a
published crate's tests still build without the repository around them.

The chapter states the shape the guard parses (a row's first cell is the
backticked name; the three tables sit under three named headings). Changing
one of those headings, or moving a row out from under them, is a change to
the guard and fails it, which is the point: the document and the code can
only move together.

What this prevents: a new engine method quietly reachable by every local
application because the server's dispatcher was generated from the
definition and nobody classified it. The guard makes "unclassified" a
failing test rather than a default.

## L2. The server owns the run loop, the drain and the lifecycle

No client calls `process()`, `receive_message()`, `start()`, `stop()`,
`pause()` or `resume()`, and none of them is a method on the wire. The
server runs the loop the mobile bridges run (100 ms), drains on every tick,
and starts and stops the engine on the operator's instruction.

This is the first invariant of the chapter and it is load-bearing in a way
that is easy to miss: the engine emits `message_received` from inside the
drain, and acknowledges and dedup-marks the message there. A client that
could drain would take messages out from under the server's routing; two
drainers would split one stream; a server that did not drain would leave
every inbound message unacknowledged while the sender retries into silence.
The Python manager's own documentation already says that a second caller of
`receive_message()` always sees `None`; a server makes that impossibility
structural instead of documented.

**The drain's return value is never relayed.** `receive_message()` returns
the core `Message` as JSON, a different shape from the `message_received`
event the engine emits from inside the same call (`id` rather than
`message_id`, a capitalised `priority`, `forwarded_from` rather than
`forward_info`, no `transport`). The FFI crate discards that return value on
purpose, and the Python manager synthesises a second `message_received` from
it and hands it to the same handler as the engine's event. A server built on
that manager drops the synthesised event and relays only the engine's. The
failure otherwise is two `message_received` notifications per message, one
of them in a shape the catalogue does not describe, and a client that
deduplicates by `message_id` cannot even pair them, because the synthesised
one has no such field.

## L3. Routing and the hold are the server's, and never reach the wire

The three routing classes (stamped, correlated, broadcast), the per-application
hold, service ownership, space scoping, method denial and the refusal of an
unlisted id at `hello` once any allow-list or deny is configured are all
rules the server applies between the engine and the socket. Nothing about
them is sent to a peer, a relay or a gateway, and nothing about them changes
an event or a frame.

The reason to say so here is the temptation to make one of them real: an
ownership claim in the service descriptor, a per-application member in a
space. Both would be a second membership system beside MLS, which
[document replication](../spec/data-sync.md) forbids for the reason it
states there, and both would mean a peer could see which applications sit
behind an identity. The server knows; the mesh never does.

## L4. Session refusals reuse the engine's taxonomy

A method before `hello` is `InvalidState`; a bad application id is
`InvalidArgument`; a wrong token, a service another application owns, a
space outside the client's allow-list, a denied method group, or an unlisted
id at `hello` once any allow-list or deny is configured, is
`PermissionDenied`. No error variant exists only on the wire.

A server that invented its own codes for these would hand every client a
second taxonomy to handle, and would break the property that a client
written against the JSON-RPC surface handles the same variants an embedded
application does.

## What the reference server owes

| | |
|---|---|
| Carrier | A Unix domain socket by default, created `0600`. A directory the server creates for it is made `0700`; one that already exists must be this user's with no group or other bits and is refused otherwise, never narrowed (the operator's `0755` directory, or `/tmp`, is not the server's to change, and a socket others can reach is not the credential the chapter names). A stale socket at the path is removed; a regular file there is refused. TCP on loopback only when enabled, with the per-launch token in a `0600` file |
| Frame limit | Raised from the WebSocket library's 1 MiB default to cover the engine's file size limit plus base64, so a media send is refused by the engine and not by a closed connection |
| Dispatch table | Checked in, classified against the two method tables, and read by the guard in L1 |
| Hold | Per application id, 256 entries, oldest dropped, each drop logged |
| Health | `GET /health` through the library's request hook, with the body the chapter shows; nothing else on HTTP |
| Policy | One JSON file (`--policy`): `spaces` (application id to glob patterns), `denied` (application id to method groups `sign_data`, `manual_mls`, `tuning`, or single wire names), and `applications`, ids with no rule of their own that are still admitted once a rule exists. Either of the first two turns on the unlisted-id rule; the third alone configures nothing, and service ownership never counts. Every denied name must be a group or an exposed method, or the file is refused at load with the entry named: a misspelled deny would otherwise deny nothing while the operator believed the signing oracle withheld |
| The drain | The Python manager hands the server both the engine's `message_received` event and the drain's synthesised copy of the same message; the server drops the copy by its shape (no `message_id`, because the drain's JSON keys the id `id`) and relays the engine's. On four of the five carriers the FFI drains inside its inbound entry point, so the manager's drain sees `None` and no copy is made; Nostr, and a message the engine releases on a later `process()` tick, do reach the drain. The seam is therefore pinned directly: a test feeds the server the engine's event and then the drain's copy of the same message and asserts one `message_received` reaches the client, and a test over two servers asserts the same over the peer stream |
| Token file | Written with its mode in one `open` (`O_CREAT` and `O_EXCL`, `0600`), never created and then narrowed, so the token is never readable by anyone else for an instant; a stale file from an earlier launch is removed first |
| Calls | Every engine call runs on the loop's default executor behind one server-wide lock, so calls are serialised and the caller rule's attribution stays exact: an event the engine emits on the executor thread reaches the loop through `call_soon_threadsafe` ahead of the call's own completion, while its connection is still the caller, and an event the run loop emits on the loop thread is never the caller's. What keeps running during a call: `process()` and the drain, every other connection's framing and `hello`, and `GET /health`; what waits: every other engine call. The cost this is for: a media send marshals `sequence<u8>` per element in pure Python, about 1.5 s per MiB (4 MiB measured at 5.9 s), and the frame limit admits 134 MiB, so without the executor nothing ticked for that long. That holds for a call slow in its Python marshalling; a call slow inside the engine (a large `data.export_raw`, an MLS operation) holds the engine's own lock, and `process()`, which the manager calls synchronously on the loop, blocks on that lock for as long as the call does. One more piece keeps attribution exact: between the executor's completion and the wakeup that records the call's result, a loop iteration or two run, and a `process()` tick landing there can emit `message_sent`, content and all, for the very id the call is about to return, with `in_call` false and the id not yet recorded. The router parks any loop-thread event that names an identifier nobody owns while a call is in flight, and routes it once the result is recorded (or the call has failed); without that the event would broadcast, which is one application's message in every other application's stream. A test pins that a 1 s call leaves `process()` ticking, `GET /health` answering and a late client's held events delivered, and that a second client's call waits; another drives the park from the executor thread and asserts the event reaches only the caller |
