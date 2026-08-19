# 0018. The data layer has two seams: the engine and the backend

**Status:** Accepted
**Shipped in:** unreleased (F2, local half)

## Context

The data layer adds a second application class on the protocol: replicated
documents any space member can edit offline, merging deterministically on
reconnect. Building it means taking on two dependencies that are unusually
expensive to change later.

The first is a CRDT engine. Merge logic is the worst component to build
in-house (it is subtle, security-relevant when it goes wrong, and has no
partial-credit failure mode), so an engine gets embedded. The engine
measured for the job costs roughly +1.5 MB on a 3.1 MB mobile binary, which
is a real number an app team may eventually push back on, and it publishes
no MSRV metadata at all, so every version bump is an empirical compatibility
question rather than a declared one.

The second is storage. The product differentiator against comparable
offerings is freedom of choice through modularity: no database vendor
lock-in, and no mandatory hosted piece. That claim is worth nothing if
"bring your own backend" is technically possible but practically a fork.

Both dependencies share a failure shape. If either leaks into a durable
surface (a public API, the UDL, a binding, a wire format), replacing it stops
being an implementation change and becomes a breaking release everywhere.
And both leaks are the kind that happen quietly, in a refactor that looks
like a simplification.

## Decision

Two seams, each with a named failure it exists to prevent.

### 1. The engine is sealed inside `offline-protocol-data`

No engine type appears in that crate's public API, in `offline-protocol`'s
re-exports, in the UDL, or in any binding. Callers see collections, opaque
byte deltas, opaque version tokens, and two escape hatches
(`export_json`, `export_raw`).

The version is pinned exactly (`=1.13.9`), not caret-ranged. The engine
publishes no `rust-version`, so MSRV compatibility is only ever an empirical
claim about one release; a caret range would let a `cargo update` silently
invalidate it.

**Failure prevented:** one leaked engine type in the FFI surface and the
engine can never be replaced without a breaking release in three languages.

### 2. Documents only ever import bytes that came out of a sealed record

The engine has open upstream issues where a malformed or semantically
inconsistent import blob panics rather than returning an error, and one of
them poisons the document's lock, making the document unrecoverable for the
process. Under the `minisize` profile the SDK ships with `panic = "abort"`,
so `catch_unwind` is not a defense there at all.

The containment is therefore structural, not defensive: every blob the
engine imports has come back out of a protocol-state record whose AEAD tag
already verified it. Corruption at rest fails the seal and lands in the
existing `Unreadable` path before the engine sees a byte. The blob-metadata
check and the `catch_unwind` in `offline-protocol-data` are a second line
for the case where the engine rejects its own output.

**Failure prevented:** an unrecoverable document, or an aborted process on
mobile, from data the SDK did not write. This is also why replication (F3)
cannot simply hand remote deltas to `import`: that is a different threat
model, and it needs its own answer.

### 2a. An accepted import is not necessarily an applied one

The engine accepts a change whose causal predecessor it has never seen and
answers `Ok`. The change is *parked*: invisible to every read, and absent
from a compacted export, until the predecessor arrives. `import` therefore
returns `Applied` or `Parked`, and any caller that can destroy history has to
branch on it.

Two rules follow, and both are load-bearing rather than defensive:

- A document whose delta log did not fully apply is **never compacted**.
  Folding it would write a snapshot without the parked changes and then
  delete the records that still hold them, which converts a gap that a
  resend could close into permanent loss. A growing delta log is the
  cheaper failure.
- A delta record that fails to read **transiently** refuses the open,
  exactly as an unreadable snapshot already did. Skipping it does not cost
  one commit; it costs every commit after it, because they all park behind
  the gap.

**Failure prevented:** a single transient read failure, or one corrupt
record, silently emptying a document at the next compaction.

### 3. Storage is a one-line runtime swap, and sealing sits above it

The backend for documents is `DataConfig::storage` in Rust and a second
`DataStore` constructor over FFI. It is a runtime choice: no rebuild, no
cargo feature, no change to any data API. Absent, documents live in the
backend protocol state already uses, which every binding ships a default
for, so the zero-configuration path is a working database and not a
homework assignment.

Sealing is decided per category *above* the adapter. An adapter is handed
sealed bytes and never sees document content, so swapping backends cannot
change the at-rest posture, even if the replacement persists to somewhere
careless.

The same trait the SDK already uses for protocol state is reused rather than
a new data-specific one, so an application implements one storage interface
and one conformance suite covers it.

Swapping mid-session writes every open document into the new backend as a
self-contained snapshot before the swap returns, and if that cannot be
written the swap does not happen at all. This is not a convenience. A delta
record only describes the change *since* the previous one, so a document that
merely kept appending deltas into the new backend would leave its history
behind in the old one; the orphan delta then parks (see 2a), the document
reads **empty**, and the next compaction deletes the orphans for good.

The migration runs in two phases, and the split is load-bearing rather than
tidiness: every document is written into the new backend first, and only once
all of those writes are durable does any document's in-memory bookkeeping
move. Interleaving the two makes a partial failure worse than no migration at
all. A document that migrated before the failure would carry bookkeeping
claiming a fresh empty log while the swap is rolled back to the old backend,
so its next flush writes sequence zero over the delta already sitting there
and everything after that delta parks at the next open. The application was
told the swap did not happen, which is what makes that loss silent.

**Failure prevented:** a refused backend swap destroying the delta log of
whichever documents were migrated before the refusal.

Note what the seam does not hide: values are sealed, but record **key ids**
are not. A backend sees `{space}/{doc}`, so space and document names are
metadata visible to whoever runs the store, and an application must not put
secrets in a document name.

**Failure prevented:** the differentiator dying by degrees, through an
adapter interface that grows a second shape, a swap that needs a rebuild, or
a backend author being handed plaintext.

### 4. Swappability is verified, not asserted

An adapter conformance suite ships with the layer, in Rust and reachable
from every binding through one FFI entry point. Green against it is the
definition of "this backend is supported".

**Failure prevented:** an adapter that returns `Ok` from every method and
still loses overwrites, merges categories, or truncates large values. Each
of those is invisible until data is missing.

### 5. A custom backend brings a logout obligation

`wipePersistedState` deletes the account directory of the *default*
provider. That is deliberate (one unlink instead of thousands, and it takes
categories a future release adds, for free), but it means a custom backend
is somewhere the bindings cannot reach. `DataStore.wipe_all()` exists for
exactly this, and the adapter documentation states the obligation.

**Failure prevented:** documents outliving the account that created them,
which is a privacy failure that looks like nothing at all from inside the
app.

## Consequences

- Replacing the engine is a change confined to one crate. Replacing it in a
  *released* fleet is still a wire-compatibility question, which is F3's
  problem, not this one.
- Every engine version bump re-runs the MSRV check and the mobile binary
  size measurement. Neither is optional and neither can be inferred from the
  changelog.
- The mobile artifact carries the engine whether an app uses it or not: two
  binding flavors would mean feature-gated UDL objects and a runtime
  checksum mismatch, which is a worse failure than the bytes. Native
  crates.io consumers opt out with `default-features = false`. Splitting the
  artifact is a release decision, never a quiet refactor.
- The conformance suite is a maintenance obligation: a new expectation of
  backends is a new check, or the promise silently narrows.

## Alternatives rejected

- **Hand-rolled merge (LWW on a Lamport clock).** The workspace's clamped
  scalar clock is not causality, and merge logic is precisely the component
  whose bugs are silent and permanent.
- **An engine with a session-based sync protocol.** It would need ordering
  and session state this layer deliberately does not have; the delivery
  ladder promises at-least-once and unordered, and CRDT semantics are what
  make that sufficient.
- **A bundled SQL or key-value engine as the storage layer.** It would make
  the thing we ship the thing apps are locked into, which is the outcome
  this ADR exists to prevent.
- **A separate data-specific storage callback interface.** Cleaner to name,
  but it forks what an application must implement and halves the value of a
  single conformance suite.
