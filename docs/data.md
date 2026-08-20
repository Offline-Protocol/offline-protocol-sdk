# Replicated Documents

The protocol's second application class. Messaging is synced events; this is
synced state: a document any member of a space can edit while disconnected,
merging deterministically when the replicas meet again.

This guide is for application authors deciding whether to build on it and how
to model their state. The wire contract is
[Document replication](spec/data-sync.md), the method-by-method surface is the
[API reference](api-reference.md#replicated-documents), and the runtime
behaviour is [Document replication](state-machines/data-replication.md) in the
state machines.

## When this is the right tool

Both classes ride one carrier, one encryption, and one set of transports. The
question is what the state is, not how it travels.

| Use a message | Use a document |
|---------------|----------------|
| A thing that happened at a time, in an order people can see | A thing that is currently true, whatever order the edits arrived in |
| The receiver should be told once | Every member should be able to read it later, including one who joins after the edit |
| The app keeps its own history | The app wants the current value and would otherwise rebuild it from a log |
| One sender, many readers | Many editors, no coordinator |

Chat, invitations, receipts and typing indicators are messages. A shared
checklist, a group's settings, a collaboratively edited note, a per-member
profile, a synced counter: those are documents, and building them out of
messages means writing merge rules by hand for every field.

Note that a document is not a database in the query sense. There is no query
language, no index, and no partial replication in this version: a space
replicates whole. Model state a member is entitled to hold in full, and keep
anything else in messages or in your own storage.

## The model

**A space is an MLS scope.** A 1:1 session or a group *is* the space, and the
roster that already exists is the membership. There is no second membership
system to keep in step, which is the failure this shape prevents: two rosters
that disagree about who is in the room.

- A 1:1 space is named by the peer's address, so the two replicas name the
  same space differently, each by the other's address. Pass the peer address
  as `spaceId`.
- A group space is named by the group id (`group:<uuid>`), so both replicas
  name it identically. Pass the group id as `spaceId`.

Inside a space are **documents**, inside a document are named
**collections**, and a collection is one of four types:

| Collection | Concurrent edits resolve as |
|------------|-----------------------------|
| `map` | Last writer wins per key. Different keys never conflict |
| `list` | Both insertions survive, in a deterministic order |
| `text` | Character-level merge, so two people typing in one paragraph both keep their words |
| `counter` | Increments add up. Two devices adding 1 offline produce 2, not 1 |

A map value is one of `null`, `bool`, `int`, `float`, `text`, `bytes`, or
`attachment`.

**A collection is identified by its name and its type together**, so one name
used with two types is two collections. Both hold their data, both replicate,
and both read back through their own accessors, but `docJson()` shows only one
of them. Nothing reports the overlap, so treat a collection name as belonging
to one type for the life of the document: `textInsert(..., 'notes', ...)` and
`mapSet(..., 'notes', ...)` do not disagree with each other, they simply do not
meet.

**Values are replaced, never merged.** A map value or a list entry is one
whole value. If two members write different values to one key, one of them
wins and neither replica ends up holding half of each. Structured data goes in
as a JSON string and merges whole, which is the trade this version makes: put
the fields that should merge independently in separate keys, not in one blob.

Names are bounded because they become record keys. A document or collection
name is 1 to 128 bytes of `A-Z a-z 0-9 . _ -`; a map key is 1 to 256 bytes and
otherwise unrestricted. A name outside that raises `InvalidArgument` at the
call that used it.

## Opening a store

Two prerequisites, and neither is storage configuration:

- `initializeMls` must have run. Documents are sealed at rest with the same
  per-install record key as every other protocol-state category, and that key
  is minted there. Before it, every method answers `DataStorageUnavailable`.
- `data.enabled` must be on. It defaults to `true`, so there is nothing to
  set; setting it to `false` makes every method answer `DataDisabled` and
  stops the capability being advertised to peers.

```typescript
import { DataStore } from '@offline-protocol/mesh-sdk';

const store = new DataStore();

await store.mapSet(peerAddress, 'shopping', 'items', 'milk', {
  kind: 'text',
  value: '2 litres',
});
await store.textInsert(peerAddress, 'notes', 'body', 0, 'Meet at the bridge');
await store.counterIncrement(peerAddress, 'stats', 'opened', 1);

await store.flush(peerAddress, 'shopping');
```

Native platforms construct the store over a live protocol instance:
[iOS](ios-integration.md#replicated-documents) and
[Android](android-integration.md#replicated-documents). In Rust the same
operations are methods on `OfflineProtocol` (`data_map_set`, `data_flush`, and
so on).

Two runnable programs are worth more than either listing. The first opens a
store, writes to all four collection types, and reopens the same records after
the engine is rebuilt; the second is the one that answers "what happens if we
both edit this":

```bash
cargo run --package offline-protocol --example replicated_notes
cargo run --package offline-protocol-data --example offline_merge
```

## Durability

Edits batch before they reach storage, so a burst of keystrokes is not a burst
of records.

Call `flush()` when the app must know a change survived a crash. The SDK also
flushes every open document when the protocol instance is dropped, and a
`data_changed` event fires **after** the change is durable, never before: a UI
that re-renders on that event is rendering state that survives a restart.

Persisted changes are folded into the document periodically, so history does
not grow without bound. Compaction runs when the delta log passes four times
the compacted document (with a 64 KiB floor) or after 1024 commits.

It switches off entirely for a document whose records did not all apply: a
change that arrived ahead of the predecessor it builds on is parked and
invisible until that predecessor turns up, and folding the log then would
write a snapshot without it and delete the records holding it. A growing log
is the cheaper failure. Compaction returns for that document when it is next
opened over a complete log.

## Replication

Replication is anti-entropy, not a stream. Each side offers the version of
every document it holds for that peer, the other answers with what is missing
and its own versions, and the exchange stops. Nothing persists about how far a
peer got: state that has to be reconciled after a crash is state that can be
wrong.

An exchange is triggered by:

- a local change becoming durable, which is pushed to the space immediately,
- an MLS session being confirmed with a peer,
- that peer being rediscovered on any transport,
- start-up, for 1:1 spaces,
- joining a group, or a member being added to one.

Offers to one peer are rate limited to one per 30 seconds. That window delays
only the reconciliation sweep: a local change does not wait for it.

Two things follow from at-least-once, unordered delivery being enough here.
The first is that nothing needs ordering or exactly-once semantics; duplicates
and reordering are absorbed by the merge. The second is that **a peer who was
away misses nothing**: the next exchange sends whatever they lack, however
long they were gone, as long as both replicas still hold the history in
between.

### In a group

A group space replicates over the group send path, so one encryption serves
the whole roster. Three consequences an application can see:

- **Every member must be on a build that speaks group replication.** One
  member that is not means no member is sent a group replication frame, because
  the ciphertext reaches everyone and that member would surface it as text.
  A single old device therefore stops group document sync for the whole room
  until it updates.
- **A change received from a group is not pushed back into it.** Anti-entropy
  still closes real gaps, and re-broadcasting would turn one edit into N²
  frames.
- **Attachment bytes do not move inside a group** in this version. References
  replicate like any other value; fetching the blob from a group member is
  refused, because the transfer needs a confirmed pairwise session and two
  group members need not have one.

## Size, and what happens at each limit

A document is bounded by one sealed protocol-state record. The limits below
exist because of that, and each one is reported rather than silent.

| Limit | Value | What happens |
|-------|-------|--------------|
| Document, compacted | 1 MiB | `DocTooLarge` at the call that breached it. The breaching change is still durable, deletions still apply, and the document accepts edits again once it is back under the cap |
| Warning | 768 KiB | `data_doc_size_warning`, while there is still room to act |
| One sync frame | 32 KiB of document bytes | The document is carried over the media path instead |
| Whole document over the media path | 4 MiB, the record ceiling | A document past it is refused at the start of the transfer, because the receiver could not persist it even if every byte arrived |
| Nowhere left to go | | `data_doc_unsyncable`, which is worth handling: the replicas will not converge, both sides keep accepting edits, and nothing else about that state looks like a problem |
| Documents a peer may name in one space | 1024 | Bounds abuse rather than product use. Documents this application creates are not counted against it |

Keep documents small on purpose. A shared list per conversation converges in
one frame; a single document holding every list a person owns eventually does
not fit in one, and the rungs below that are slower every time.

## Attachments

A document can name a blob it does not contain: a SHA-256, a size, and enough
to display the thing. The bytes never enter the document and never enter
protocol state, because a document is bounded by one sealed record and a layer
that inlined blobs could not carry the blobs people actually send.

**Your app owns the bytes.** The SDK never kept a copy, so when a peer asks
for one you are asked, through `data_attachment_requested`. Answer with
`provideAttachment` or refuse with `declineAttachment`, and answer either way:
a reference outlives the bytes it names, so a peer holding a reference and no
blob is ordinary, and without a refusal the asking side cannot tell that from a
slow radio and shows somebody a spinner forever.

The full surface, with worked code, is in the
[API reference](api-reference.md#attachments).

## Storage

Documents live wherever protocol state already does, so a new application gets
a working store with no storage setup at all: every binding ships a default
provider.

Putting them somewhere else is one line at construction rather than a rebuild,
a cargo feature, or a change to any data API. In Rust it is a config field:

```rust
let config = ProtocolConfig::builder("my-app", "default")
    .data_storage(Arc::new(MyBackend::open("documents.db")?))
    .build()?;
```

and on the bindings it is a second constructor:
`DataStore.withStorage(protocol:storage:)` on Swift,
`DataStore.withStorage(protocol, storage)` on Kotlin,
`DataStore.with_storage(protocol, storage)` on Python. The React Native
JavaScript surface has no equivalent: an app there gets the default provider
its native bridge already ships, and reaches this seam from native code if it
needs to.

Sealing sits above that seam. An adapter is handed sealed bytes and never sees
document content, so a custom backend cannot weaken the at-rest posture even by
accident. Verify one with `runStorageConformance(provider)` before shipping it:
an adapter that returns success from every method can still lose overwrites or
merge categories, and the suite is what catches that. Reference adapters for
Swift, Kotlin and Python live in
[`examples/storage-adapters/`](../examples/storage-adapters/README.md).

One obligation comes with a custom backend: call `wipeAll()` on logout.
`wipePersistedState()` clears the account directory of the *default* provider,
which a custom backend is not inside, so documents would otherwise outlive the
account that made them.

## Events to handle

| Event | Handle it because |
|-------|-------------------|
| `data_changed` | The change is durable. Re-render here |
| `data_doc_size_warning` | The cap is a cliff otherwise, met for the first time as a failed write |
| `data_attachment_requested` | Only your app has the bytes. Answer or decline |
| `data_attachment_received` | The bytes arrived and matched the hash that asked for them. Store them where you keep files |
| `data_attachment_unavailable` | The fetch ended without bytes: `declined`, `timeout`, `evicted`, `hash_mismatch`, `peer_gone`, or a transfer failure. Stop the spinner |
| `data_doc_unsyncable` | Two replicas that will not converge, and nothing else reports it |

## What this version does not do

Each of these is a decision rather than an omission, and the reasoning is in
the [design record](spec/data-sync.md) or the ADRs.

- **No deletion tombstones.** `deleteDoc` removes the document here, and the
  peer's next offer recreates and refills it, because a peer cannot tell a
  deleted document from one this device has never seen. To remove content from
  both replicas, empty the document: deletions *inside* a document replicate
  like any other change. The same applies to `wipeAll()`, which is durable only
  once replication has stopped.
- **No query language and no partial replication.** A space replicates whole.
- **No hosted component.** Relays and gateways carry sync frames as opaque MLS
  ciphertext they cannot read, and nothing in this layer requires a server.
- **No blob carriage in groups**, as above.
- **No garbage collection for blobs.** The SDK does not know which references
  still exist across every space, so deciding when your stored bytes are
  unreferenced is your app's job.

## Where to read next

| Question | Document |
|----------|----------|
| What exactly is on the wire | [Document replication](spec/data-sync.md) |
| What the runtime does, state by state | [Document replication state machine](state-machines/data-replication.md) |
| Every method and event | [API reference](api-reference.md#replicated-documents) |
| Why an engine and a backend seam | [ADR 0018](adr/0018-data-layer-engine-and-storage-seams.md) |
| Why remote imports are contained | [ADR 0019](adr/0019-remote-document-imports-are-contained-not-trusted.md) |
| What a hostile space member can do | [Threat model, R11](security/threat-model.md) |
| Writing a storage backend | [Bridge contract C11](bridges/README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) |
