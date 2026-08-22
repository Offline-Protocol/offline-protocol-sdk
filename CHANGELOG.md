# Changelog

All notable changes to the Offline Protocol SDK are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog covers
everything after the **v0.7.1** release.

This file holds unreleased changes and the current release. Older releases are
archived by series under [docs/changelog/](docs/changelog/); see the
[archive index](docs/changelog/README.md).

## [Unreleased]

### Added

- **`offline-protocol-core` compiles without `std`.** The crate now builds for
  bare-metal targets with `--no-default-features`, which is what a constrained
  leaf node links: a Cortex-M door lock, sensor or mains-powered relay that
  speaks the protocol but cannot host the engine. `std` is a feature and it is
  on by default, so the published API and every existing consumer are
  unchanged.

  What `std` gates is exactly what the platform has to supply and a bare-metal
  target does not: a wall clock (`Timestamp::now`, `WallClockTimestamp::now`),
  a monotonic clock (`LocalInstant`), entropy (`MessageId::new`, and
  `Message::new` and `MessageBuilder` with it), and threads (the `sync`
  module). Everything that parses, validates, re-encodes or compares is present
  in both configurations, which is the half a leaf node that only forwards
  uses. A node that answers also mints, and `Message::from_parts` in this same
  release is what lets it: build messages from wire-supplied parts, via
  `MessageId::from_bytes` and `Timestamp::from_millis` on that path.

  Two field types changed spelling to `MetadataMap`, which under `std` **is**
  `HashMap<String, String>` exactly as before, and is a `BTreeMap` without it.
  Nothing moves on the wire: JSON objects are unordered by definition and the
  binary v1 codec carries metadata as an ordered `Vec<(String, String)>`.
  `chrono` is no longer a dependency of this crate. See
  [ADR 0020](./docs/adr/0020-core-compiles-without-std.md).

- **The protocol layer's cost on a Cortex-M33 is measured, not estimated.**
  `tools/embedded-footprint` links two firmware images for
  `thumbv8m.main-none-eabihf` and reports the difference: **about 95 KiB of
  flash (97,152 bytes) and no static RAM** beyond the heap a node provisions
  for itself. That is roughly 6% of an xG24's 1536 KB. CI prints the table on
  every run.

  The figure is the protocol layer alone. It excludes signature verification
  and payload unsealing, the radio driver, an RTOS and key storage, and it is
  not the engine, which is `std` and `tokio` bound and does not build for this
  target at all.

- **`offline-protocol-sealed`, a new crate holding what both ends of a sealed
  conversation must agree on.** A phone and a leaf node run different MLS
  implementations ([ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md)) and
  are not allowed to disagree about anything outside them: the
  `EncryptedMessage` envelope and its compact codec, `derive_address`, the
  domain-separated canonical signing payload every signature is taken over, and
  the sender-ratchet bounds a session is configured with. All four were in
  crates that need `std`. They now live in one crate that builds for bare metal,
  and everything else re-exports or delegates to it, so there is exactly one
  implementation of each in the workspace.

  `tools/mls-interop` previously carried its own copies of the derivation rule
  and the ratchet constants, with nothing pinning them to the SDK's. It now
  imports them, and a guard test reads the harness source and fails if the
  copies come back. See
  [ADR 0022](./docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md).

- **Two more pieces join that layer: the 1:1 control-frame prefixes and
  `KeyPackagePayload`.** Both were private to the engine, and both are things
  the two ends of a sealed conversation have to agree on while building them
  with different MLS implementations. A frame's type is the prefix its content
  begins with, so two ends that disagree about one do not have a conversation;
  and the key package payload is the only channel in this protocol by which
  capabilities are advertised, which is why a device that cannot mint one is
  served the floor forever.

  `offline-protocol-sealed` gains `prefixes` (the six a pair speaks: key
  package, Welcome, encrypted, the two confirmation frames, and the encrypted
  confirmation that only ever travels inside an envelope), plus
  `KeyPackagePayload` and `MLS_ENVELOPE_COMPACT_V1`. **Reservation stays in the
  engine**: `INTERNAL_PREFIXES` is what refuses application content beginning
  with a reserved prefix, and it is still generated from the one macro
  invocation that names them, so adding a prefix is still a single-line change
  in a single place. Group, connection, relay, presence and sealed-body
  prefixes stay in the engine too, because none of them reach a device and the
  alternative puts relay vocabulary in an image budgeted in kilobytes.

  Pure relocation: no wire byte, no JSON field, no error string and no FFI
  signature changes, and every engine use site is untouched because the
  constants and the type are re-exported under their existing names. Two new
  guard tests pin the prefix literals byte for byte and prove none of them is a
  prefix of another, which is a live near miss between `__MLS_ENC__` and
  `__MLS_ENC_CONFIRM__`.

- **What a leaf node owes at pairing is specified** in
  [docs/spec/leaf-provisioning.md](./docs/spec/leaf-provisioning.md). A leaf is
  a peer rather than a class of peer: same frames, same envelope, same trust
  gates, no second sealing path. What is genuinely different is four
  obligations that a passing build does not reveal, and each is stated with the
  failure it prevents. A static artifact carries `{address, pubkey}` and never
  a key package, because an init key is single use and a sticker is not. A key
  package is minted from a **supplied** time, because an implementation that
  cannot read a clock stamps a validity window at the Unix epoch and the peer
  refuses it as expired, so a device that ships that way never pairs at all.
  State is persisted **before** a frame that advanced it is emitted, because a
  device that answers and then loses power comes back and reuses an AEAD nonce.
  Entropy comes from real hardware, because MLS key generation is exactly as
  strong as what that source returns.

  The chapter also specifies the never-committing profile (what a leaf emits,
  what it accepts, and what it must never emit), and states that a phone-driven
  rekey reaches a device as a key package with `session_reset` set rather than
  as an unsolicited Welcome, which is the sequence a device has to survive for
  post-compromise security to arrive at all.

- **The leaf profile in [capability
  negotiation](./docs/spec/capability-negotiation.md)**: what a minimal device
  advertises, and the two rules that are easy to get backwards on a part where
  every kilobyte is argued over. A device that advertises nothing still
  interoperates, because empty lists select the floor and the floor is a
  complete conversation. And parsing stays unconditional on a device too: a
  leaf that decodes only the envelope form it advertised drops frames from a
  peer that legitimately believed it capable.

- **Two threat-model entries for the device class**:
  [A8, the provisioning-time adversary](./docs/security/threat-model.md), who
  handles a device or its label before its owner does, and has no
  cryptographic answer because every cryptographic check passes (the key on the
  swapped label does derive to the address on that label); and R12, which says
  plainly that boundary 3's "platform secure storage holds" assumption means an
  OS keystore on a phone and whatever the part provides on a microcontroller.
  One device, one key: a fleet sharing an identity key turns one extraction in
  a laboratory into every unit's identity.

- **`offline-protocol-leaf`: a constrained device that speaks this protocol as
  a real peer.** A door lock or a sensor with a few hundred kilobytes of flash
  and no operating system now runs RFC 9420 MLS through mls-rs and holds an
  end-to-end encrypted conversation with a phone under the same guarantees a
  phone gets. Same frames, same envelope, same trust gates, no second sealing
  path and no reduced properties, which is the decision
  [ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md) took and this crate
  implements.

  `LeafDevice` is a frame-level state machine rather than a bag of primitives:
  an inbound message goes in, the frames to send and what happened come out. It
  runs the **never-committing member** profile, so the phone creates the group
  and issues every commit while the device joins, opens, answers and persists.
  Per-commit cost on the device is two elliptic-curve operations and
  per-message cost is symmetric only.

  What it refuses is the point. Every control frame must carry a signature
  whose key **derives to the address the frame claims**, and an identifier that
  is not an address is the same refusal rather than a skip, because a claim
  with no derivation to check is the bypass. A Welcome must name the peer that
  signed it, name the group this pair would build, spend **the key package this
  device minted for that peer**, and then actually join the group its body
  claimed. That third one is what separates a peer from anyone who overheard
  it: a key package rides in a frame that is signed but not encrypted, so a
  copy taken off the air is as spendable as the original and satisfies every
  other gate honestly. Checked before the join, because joining spends the init
  key, and a package burned by a listener leaves the peer it was minted for
  holding a Welcome that no longer opens. The group must still be a pair every
  time the device uses it, re-read from the roster and derived rather than
  read, because a commit changes the membership without changing the group id
  and every commit here is the peer's to make. Checking only on the commit
  would check at the one moment whose answer cannot be kept: it is applied and
  durable by the time there is a roster to read, so the refusal is a returned
  value that no reboot survives, and the device would seal its next message
  into the widened room and call it an ordinary session. A sealed frame's MLS
  sender must be the peer the frame came from, commits included. A confirmation
  probe is answered only by a device that still holds a session it can load and
  that is still this pair, because a peer confirms on that answer and
  flushes into it, and an inbound acknowledgement is never acted on at all,
  because a leaf emits those and never probes, so every one that arrives is
  unsolicited and treating it as proof of a session would let any keypair
  holder assert one. A reset frame is acted on once, so a captured one is not a
  repeatable session teardown. Underneath all of them, a frame addressed to
  another node is ignored before a prefix is read: a signature covers the
  recipient rather than checking it, and a sealed frame carries none at all, so
  without that gate an overheard key package mints a private init key nobody
  asked for and a captured frame with its recipient rewritten is still acted
  on. Every one of these is covered against a real OpenMLS phone in the same
  process, which is the only kind of test that catches a default in one library
  the other refuses.

  **What it keeps is bounded.** Prior-epoch records are trimmed to a window
  rather than kept forever, which bounds both the flash they occupy and how far
  back a stolen device reads; unpairing erases them along with the session and
  with the key package minted for that peer, so neither epoch secrets nor an
  unspent init key outlive the erasure an owner asked for. Peer records and
  unspent key packages are bounded too, and a full peer table refuses a
  stranger rather than evicting somebody the owner paired with, because
  producing a frame that derives to its own address costs an attacker nothing.
  Every operation that advances state takes `&mut self`, so two seals racing
  into one AEAD nonce is a compile error rather than a rare one.

  **Persist-before-emit is structural, not documented.** Every operation that
  advances ratchet state writes through `LeafStore` and only then returns the
  frame, so a store that fails produces an error and no frame at all. A device
  that emitted first would come back from a power cut and reuse an AEAD nonce,
  which is a confidentiality failure rather than a lost message. A test arms a
  failing store and asserts both that nothing is emitted and that the write was
  actually attempted, so it cannot pass by short-circuiting earlier.

  The seam is atomic per entry rather than across a set, so what a cut lands
  between is chosen rather than left to chance. Prior-epoch records go down
  before the state, because a state with records it does not yet reference
  costs nothing while the reverse loses the out-of-order tolerance a lossy
  radio needs. The **epoch marker mls-rs sequences against travels inside the
  state entry**, because ordering cannot make those two safe in either
  direction: a marker that reached flash without its state refuses every commit
  that follows, permanently, and the device goes deaf to its peer until that
  peer drives a full reset. A separate high-water record survives the state and
  bounds the erasure sweep on unpair, which is the one job the in-state marker
  cannot do.

  Four obligations stay with the integrator, and the API is shaped so none can
  be forgotten silently: every entry point needing a clock takes
  `now_unix_secs` (a device that lets an MLS library read a clock it does not
  have stamps 1970 and is refused as expired, so it never pairs at all), the
  crate registers no `getrandom` backend (firmware wires the part's hardware
  entropy source, and key generation is exactly as strong as what it returns),
  and `LeafStore` must be atomic per entry. The fourth is **authorization**: a
  session proves who a peer is and never that the owner meant them, since any
  address in radio range can complete a pairing, so firmware decides when the
  radio accepts one and what a given peer's messages may actuate. A lock that
  opens for whatever arrives on an established session opens for anyone patient
  enough to pair with it. `LeafDevice::peers` is how firmware audits what a
  device accumulated and `unpair` is how it removes one, and every route to a
  session puts the peer on that list, because a session firmware cannot see is
  one it can neither review nor revoke.

- **`Message::from_parts` in `offline-protocol-core`**, which is
  `Message::new` with its two ambient inputs, the clock and the entropy, made
  explicit. ADR 0020 made core build without `std` on the reading that a
  constrained node "receives frames rather than minting them". That is true of
  a node which only forwards and false the moment one answers, so without this
  a bare-metal node could not produce a `Message` at all. `Message::new`
  delegates to it, so there is one struct literal rather than two that drift.

### Changed

- **The envelope codec and `GroupId::new` now return `SealedError`** rather
  than `MlsError`, having moved into `offline-protocol-sealed`. Nothing else
  changes: `From<SealedError>` exists for both `MlsError` and the engine's
  `Error` and passes the inner message through, so every rendered error string,
  every FFI error code and every wire byte is what it was. This is visible only
  to Rust code that matches on the error type of `EncryptedMessage::from_bytes`,
  `EncryptedMessage::from_base64` or `GroupId::new` directly.

- **The embedded footprint harness measures the shipping crate.** Its leaf
  image drove mls-rs directly, so it linked neither the envelope codec, nor the
  control-frame signing, nor the address derivation: it priced an image nobody
  could ship. It now runs `offline-protocol-leaf`, and the whole-image figure
  moved from **391.3 KiB to 445.6 KiB** of flash, a little over a quarter of a
  1536 KiB xG24.

  The 400 KiB figure in [ADR 0021](./docs/adr/0021-a-leaf-node-speaks-mls.md)
  was a decision gate, set to answer whether MLS on a leaf node was viable
  before anything was built, and it did that job. It is not a budget the
  shipping image is held to, and the recovery lever recorded beside it is still
  worth more than the growth: about 111 KiB of the image is P-384 and P-256
  arithmetic that nothing uses, linked because the crypto provider keeps all
  four curves in one enum with no feature gating.

  The `leaf-min` image is gone. It priced application messages with the
  resilience features off, and once the workload moved onto the crate, which
  requires all four mls-rs features, cargo's feature unification made it
  measure the same bytes as `leaf`. A row reporting a number for a
  configuration nobody can build is worse than no row.

## [0.23.0] — 2026-08-20

> **The protocol carries state now, not only messages.** A new data layer adds
> offline-first documents that any member of a space can edit while
> disconnected and that merge deterministically when replicas meet again, both
> between two peers and across a whole group roster. Messaging is synced
> events; this is synced state.
>
> **It ships on by default.** `data.enabled` is `true`, the capability is
> advertised in key packages, and protocol state gains three sealed categories.
> An application that never opens a `DataStore` still pays nothing: nothing is
> written until a document is written and nothing is sent until one is shared.
> What is visible either way is the negotiation, so peers can see that this
> build speaks document sync. Set `data.enabled` to `false` if you would rather
> the layer refuse outright. One obligation comes with a custom storage
> backend: call `DataStore.wipeAll()` on logout, because `wipePersistedState()`
> only clears the default provider's account directory. Start at
> [docs/UPGRADING.md §16](./docs/UPGRADING.md#16-replicated-documents-are-available-11-and-in-groups-v0230).
>
> **The routing layer that never routed anything is gone.** A learned-route
> table, a path scorer and an adaptive-TTL calculator sat between DORS and the
> mesh, complete with a UniFFI surface, native bridge code on both platforms
> that fed it on every BLE connect and every assembled message, and a
> 30-second cleanup timer. Nothing read any of it: forwarding has always chosen
> among the neighbors a device can address right now. Deleting it removes about
> 1,800 lines from the router crate plus its whole bridge apparatus, and makes
> the architecture documentation describe what actually delivers messages.
> **This is breaking on every binding** — see
> [docs/UPGRADING.md §15](./docs/UPGRADING.md#15-the-learned-route-api-is-gone-v0230).
>
> **The mobile binary grows by about 1.3 MB**, measured on the shipped iOS
> artifact under `minisize` (3,127,616 to 4,447,160 bytes, aarch64-apple-ios).
> The document engine is all of it; deleting the routing layer gives back
> 18,584 bytes of that.

### Added

- **Documents can carry attachments, and a document too large for a frame now
  gets across.** Two holes closed by the same seam.

  A document can hold a reference to a blob it does not contain: a SHA-256, a
  size, and enough to display the thing. The bytes never enter the document
  and never enter protocol state, because a document is bounded by one sealed
  record and a layer that inlined blobs could not carry the blobs people
  actually send. They travel the media path instead, which already does
  windowing, per-transport chunk sizing, ACK and retry.

  The reference is one whole value, replaced rather than edited, so two people
  attaching different blobs to the same key resolve like any other value and
  neither replica ends up holding a hash from one beside a size from another.
  A member on an older build reads the value as absent rather than rendering a
  hash to a person as text.

  Fetching is pull, and the application is in it, because the SDK never kept
  the bytes: a peer's request surfaces as `DataAttachmentRequested`, answered
  with `provideAttachment` or refused with `declineAttachment`. Please
  answer it either way. A reference outlives the bytes it names, and without a
  refusal the asking side cannot tell a peer that lost the file from one on a
  slow radio, so it shows somebody a spinner forever.

  Arriving bytes are checked against the hash that asked for them, not against
  anything the sender says about them, and bytes nobody asked for are dropped.

  Neither side sees these transfers as files. No progress, no completion, no
  failure, in either direction: nobody started that download and nobody
  attached that file. The rule binds from the first chunk rather than the
  last, because the marking rides chunk 0 and a receiver that has not seen it
  yet has to withhold rather than guess.

  The second hole was quieter and worse. A document whose catch-up exceeded
  32 KiB was warned about and dropped, so two replicas sat there accepting
  edits and diverging with a log line as the only evidence. Such a document
  now travels the same media path, and one that genuinely cannot be replicated
  is reported through the new `DataDocUnsyncable` event instead of vanishing
  into a log.

  Negotiated as a third `data_versions` entry, appended, never a bump. A peer
  without it replicates perfectly well and has no idea what a data-purposed
  transfer is, so it would hand its user a CRDT snapshot as a downloaded file.
  Blob carriage is 1:1 in this release: it rides a transfer to a confirmed
  pairwise session, and two members of a group need not have one with each
  other. References themselves replicate in groups like any other value.

  What a transfer is, is decided by its first chunk and cannot be revised
  afterwards. A duplicate chunk 0 that disagrees with the one that opened the
  transfer ends it rather than rewriting it: the marking that makes a transfer
  invisible would otherwise be a field an authenticated peer could flip
  mid-flight, handing a person a CRDT snapshot as a download or turning their
  own download invisible and never telling them how it ended.

  A document-layer transfer is refused rather than degraded when it cannot be
  sealed. The marking travels inside the encrypted chunk and nowhere else, so
  on the plaintext opt-out path it would simply not arrive.

  The SDK's own transfers take at most one of the two per-peer transfer slots,
  leaving one the application can always reach. A snapshot and an answered
  blob request are separate errands to the same peer, and both slots filled by
  invisible transfers would fail the application's own send with a limit whose
  cause it has no way to see.

  Every road that ends a fetch without bytes now reports it, including a peer
  being blocked, coming back without the replication capability, or being
  forgotten, whether on its own or in the wholesale eviction that the bound on
  remembered peers triggers (`peer_gone`). A fetch gets exactly one such
  report: a decline that races its own arriving bytes is not followed by a
  second failure for the transfer it abandoned.

  The wire contract now has its own chapter, [Document
  replication](./docs/spec/data-sync.md), with frozen conformance vectors
  beside it.

- **Documents replicate in groups, encrypted once for the whole roster.** A
  space named after a group id replicates with that group: the roster is the
  replica set, membership is the existing MLS roster, and there is no second
  membership system to disagree with it. A change is encrypted once and fanned
  out per member on the same ladder a group message takes, so a group of ten
  costs one encryption rather than ten.

  Everything the 1:1 layer established carries over unchanged, because it is
  the same code: the same frame family, the same version exchange, the same
  catch-up ladder, and the same import containment. What is new is that a
  space now has more than one other replica in it, which the three rules below
  exist to handle.

  **A change received from the group is never pushed back into it.** One group
  ciphertext already reached every member; re-broadcasting would turn a single
  edit into N² frames and get worse as the group grows.

  **Version offers and their answers are addressed to one member.** Reconciling
  with a peer is a conversation between two devices even inside a group, so
  every other member is spared traffic about a question they did not ask. Only
  a local commit goes to the roster.

  **A member that cannot intercept these frames closes the gate for
  everyone.** This is negotiated as a second `data_versions` entry rather than
  a version bump, and the distinction matters: an install shipping the 1:1
  layer replicates 1:1 perfectly well and has no group interception at all, so
  a group replication frame would surface to its user as literal text. One
  ciphertext serves the roster, so one such member means nobody is sent one.
  Because members of a group never exchange key packages with each other, an
  inviter attests the capability on the Add commit and in the Welcome, the way
  it already attests rich-payload support; a later direct exchange always
  overrides an attestation.

  A group space reconciles when a member is invited, when this device joins,
  and whenever a member is rediscovered. There is no cold-start sweep: a local
  commit is already pushed when it happens and the per-member outbox carries it
  across a restart.

  Replication frames are never sent over the relay broadcast path, which exists
  to produce a per-recipient delivery report about an application-facing
  message; a replication frame has no such identity to report on.

  Because a group has one sender ratchet per epoch, an addressed frame still
  advances the generation every *other* member has to reach, and MLS refuses a
  generation too far ahead of the last one a receiver saw. A sender therefore
  sends one frame to the whole roster after enough addressed ones, which keeps
  every member's ratchet within reach; ordinary group chat does the same job,
  so this only ever fires in a group that is purely replicating documents.
  Without it a group could quietly stop delivering a talkative member's
  messages, chat included, until a commit rotated the epoch.

- **Documents replicate between peers, over the delivery ladder that was
  already there.** Two devices with a secure session converge on the documents
  they share: changes made offline on both sides merge on reconnect, and a
  change committed while connected reaches the other side immediately.

  There is no second delivery path and no sync protocol. A sync frame is an
  ordinary message, so it inherits the retry ladder, the durable outbox,
  deduplication, park-and-probe, and re-sealing after a re-key, none of which
  needed new code: document changes are idempotent and commutative, which
  makes at-least-once and unordered delivery exactly sufficient rather than
  something to work around.

  A space replicates with the peer whose address names it. Frames never carry
  a space name — a receiver derives it from the authenticated sender — so a
  peer structurally cannot reach a document shared with somebody else. A space
  named anything else stays local.

  Negotiated as `data_versions` in the key package, the fourth capability in
  the family after `wire_versions`, `env_versions` and `rich_versions`. A peer
  that does not advertise it is never sent a frame, and nothing falls back to
  anything unsealed.

  **Changes arriving from a peer are contained rather than trusted.** MLS
  establishes who sent a blob and says nothing about its shape, and the CRDT
  engine has open upstream defects where a malformed change aborts the process
  instead of returning an error — with no unwinding to catch it on the mobile
  profile. So blobs are judged before the engine sees them, frames are bounded
  at 32 KiB, and the digest of a blob about to be imported is written to disk
  first: one that does end the process is refused when the sender retries it,
  which is the difference between one crash and a crash on every launch. The
  reasoning, and the residual risk that remains, are recorded in
  [ADR 0019](docs/adr/0019-remote-document-imports-are-contained-not-trusted.md)
  and [threat model R11](docs/security/threat-model.md).

  A document too large to catch up inside one frame is reported rather than
  sent; carrying one over the media transfer path is not yet implemented.
  Deletion has no tombstone yet, so deleting a document from a replicated
  space is local cleanup that the next offer undoes; empty the document to
  retire its contents on both sides.

  **One known gap.** Replicas that stay in contact converge. Replicas
  separated by a partition that outlives a compaction may not: compaction
  deletes history, and a peer that edited from below the deleted point holds
  changes whose ancestors are gone on the other side, which no frame can
  carry back. Both sides keep their own edits, the refusal is logged, and
  neither aborts. It closes when the engine's import hardening lands.

- **Replicated documents: a second application class on the protocol.** A new
  `offline-protocol-data` crate adds offline-first documents that any member of
  a space can edit while disconnected, merging deterministically when replicas
  meet again. Messaging is synced events; this is synced state. Collections are
  `map`, `list`, `text` and `counter`, reached through a new `DataStore` object
  on every binding.

  The local half — documents, storage, caps and compaction — landed first, and
  1:1 replication followed in this same release (see the next entry), so
  `data.enabled` ships **defaulting to `true`**. It was `false` while the layer
  could store documents but not replicate them, because advertising a
  capability with no sync behind it invites peers to expect a sync that never
  comes. That gap closed before the release did.

  Three properties are worth knowing up front:

  - **Zero storage setup.** Documents persist through the seam the SDK already
    runs on, and every binding already ships a default provider, so the layer
    works out of the box like any embedded database.
  - **The backend is swappable, in one line.** Construct the store with a
    provider (or set `DataConfig::storage` in Rust) and documents live in
    SQLite, RocksDB, files, or a corporate store, while protocol secrets stay
    where they are. It is a runtime choice: no rebuild and no build flag.
    Sealing sits above that seam, so a custom backend is handed sealed bytes
    and never sees document content. An adapter conformance suite ships with
    it — `runStorageConformance(provider)`, green means supported — plus a
    SQLite reference adapter per binding under `examples/storage-adapters/`.
  - **An application can always leave.** `docJson()` (plain JSON) and
    `exportRaw()` are part of the v1 API, not a courtesy added later.

  Documents are sealed at rest like every other protocol-state category, under
  three new categories (`data_docs`, `data_delta_log`, `data_spaces`), and are
  capped at 1 MiB compacted with a `data_doc_size_warning` event at 768 KiB.
  Passing the cap raises `DocTooLarge`: the breaching change stays durable and
  deletions keep working, so a document can be brought back under the cap.

  Switching backends while documents are open migrates each open document into
  the new backend before the call returns, and fails without switching if that
  cannot be written: a delta record only describes the change since the
  previous one, so a document that merely kept writing deltas would leave its
  history behind and read back empty. A swap that fails partway leaves nothing
  moved, in storage or in memory, so a document migrated before the failure
  cannot go on to overwrite its own delta log in the backend the swap was
  rolled back to. `DataStore.wipeAll()` reports a backend that refused a
  delete rather than answering success, since it is the logout path and a
  partial wipe has no other symptom. Individual values are capped at 1 MiB for
  the same family of reasons.

  **A custom backend brings one obligation:** `wipePersistedState()` clears the
  *default* provider's account directory, which a custom backend is not inside,
  so call `DataStore.wipeAll()` on logout as well. Stop the engine before
  wiping: there are no deletion tombstones, so a wipe on a running engine with
  live sessions is undone by the peer's next version offer, which recreates and
  refills every document with no error and no event.

  Native Rust consumers who only want messaging can opt out of the engine
  entirely with `default-features = false`; the mobile artifact carries it
  either way, because two binding flavors would mean a runtime FFI checksum
  mismatch rather than a build error. See
  [ADR 0018](./docs/adr/0018-data-layer-engine-and-storage-seams.md).

- **Transport choice can now ask where the recipient is.** The engine keeps a
  per-recipient reachability table: `(recipient, carrier, claim, source,
  recorded_at)`, written by the producers that already existed (a carrier's
  `recipient_unreachable` verdict and a relay presence answer) and read at the
  two seams that previously had to guess.

  What changes in practice: **a recipient this device holds a live mesh link to
  is now sent to over that link**, even when an infrastructure carrier is up
  and scoring would have put it first. Previously an online sender standing
  next to the recipient routed to the relay, waited for it to answer that the
  recipient was not there, and only then offered the frame to the neighbour who
  had been addressable from the first instant. The link is checked live rather
  than remembered, and a refusal falls through to ordinary selection, so a
  marginal radio cannot strand a message another carrier could take.

  And **a carrier that has said it cannot reach someone stops counting as a way
  to reach them**, for as long as the fact stands. Verdicts stand for ten
  minutes (the unreachable-probe escalation cap), presence answers for five;
  after that a fact reverts to unknown, which means exactly today's behaviour.
  Decay is deliberate: a remembered "unreachable" that never expired would keep
  a path shut long after the recipient came back, and because nothing is ever
  settled by a claim (only by the recipient's end-to-end acknowledgement or
  terminal outbox expiry), the worst a stale or lying claim can cost is latency.

  Three properties bound the change. Absent facts mean today's behaviour
  byte-for-byte, so a device that never receives a verdict routes exactly as it
  did. Live mesh links are never stored, only queried, so there is no stale-link
  failure to introduce. And a carrier that produces no facts keeps its blanket
  claim — Nostr cannot report per-recipient delivery at all, so the
  mixed-neighbourhood residual stays open there by design rather than closing by
  accident.

  DORS is untouched. It remains the recipient-blind scorer, because pushing a
  hard per-recipient fact into a seven-factor weighted sum would make it
  tunable, and a mis-tuned weight would override it silently.

- **Parking works for any carrier's verdict, not just the relay's.** The
  machinery was always keyed to the `recipient_unreachable` token rather than to
  the relay, and the bridges' failure entry points already shared one boundary;
  what was missing was which carrier had spoken. Failure reports now carry that,
  so a Reticulum gateway reporting the verdict parks the message, offers it to
  the mesh and schedules the probe exactly as the relay does. This is the
  behaviour a gateway's Verdict verb plugs into.

### Fixed

- **A local document edit could be stranded by a storage write that failed
  once and then recovered.** Edits pending when a remote change arrives are
  flushed first, so they leave on their own delta rather than being folded
  into the imported change and suppressed as an echo toward the one peer they
  were owed to. That pre-flush was reported and stepped over for every error
  alike, but the delta-write failure rewinds the commit back into the pending
  set, and the import's own flush then performs exactly the fold the pre-flush
  exists to prevent. A failed pre-flush followed by an import that applied now
  offers versions, so the peer asks for what it is missing instead of both
  replicas believing they agree. `DocTooLarge` is told apart from the rest: on
  that path the delta was written, pushed and announced before the size
  verdict failed, so nothing is pending and the old message said the opposite
  on the one document an operator is most likely to be reading about.

- **A remote change applied into a document over its cap was reported as
  refused.** The import flushes what it applied, and that flush answers
  `DocTooLarge` once the document is over its cap: the change is written and
  the size verdict that follows it is what failed. Reporting it as an error
  logged an applied change as `Remote change refused`, skipped the space
  record, and withheld the stranded-edit offer above, which is gated on the
  import having applied. That is the case the offer matters most in: the one
  edit a document past its cap still accepts is a deletion, which is its route
  back under. The import now answers `Applied`, and `Err` means the change is
  not durable. `flushAll()` drew the same distinction on shutdown, where it
  logged `Failed to flush document` for a document whose change had reached
  disk; it no longer does.

- **Messages sent over an explicitly chosen transport skipped the negotiated
  binary wire codec.** `send_via_transport` never stamped it, while the
  selection path did, so a peer known to support the binary codec silently fell
  back to JSON whenever a send bypassed selection. Both paths now share one
  stamping helper. Visible only as larger frames on the wire, with no error and
  no event, which is why it survived.

- **A group created offline was titled `group:<uuid>` on the relay once the
  device reconnected.** Only `create_group` passed the name into the relay
  registration frame, and it only sends when Internet is up at that moment.
  The reconnect re-sync and `request_group_relay_registration`, which register
  an offline-created group for the first time, sent no name; the bridge
  translator then substituted the group id, and the relay keeps the first name
  it sees for a mesh group id and echoed it back to every member as the
  group's title. A registration frame now falls back to the name in local MLS
  group metadata whenever the caller has none.
- **A member who joined a group by invite held no copy of its name, so its own
  relay registration could rename the group for everyone.** The bridge sends a
  `CreateGroup` for every member's registration, not just the creator's, so the
  member who reconnects first is the one whose name the relay keeps. MLS
  `join_group` put the Welcome's name on the group info it returned and
  persisted nothing, leaving a joiner with no name to send and the group titled
  `group:<uuid>` for the whole roster whenever a joiner reconnected first. The
  Welcome's name is now stored alongside the roles and the creator of record,
  which also carries it on down the invite chain: an invite sources the name it
  sends from the inviter's stored metadata.

### Removed

- **The gradient-routing and path-selection layer, in full.** `PathSelector`,
  `GradientRoutingTable`, `RouteEntry`, `GossipConfig`, `PathConfig`,
  `AdaptiveTtlCalculator`, `RelayInfo` and `RelayManager` are deleted from
  `offline-protocol-router`, along with the constants module that existed only
  to serve them. `RelayConfig`, `RelayPriority` and `RelayRole` stay: they are
  the vocabulary for whether this device forwards, and the standing itself is
  decided by the forwarding governor from traffic actually carried.

- **The routing surface on every binding.** `learn_route`, `get_best_route`,
  `get_all_routes`, `has_route`, `remove_neighbor_routes`,
  `cleanup_expired_routes`, `get_routing_stats` and `update_routing_config`,
  plus the `RouteEntry` / `RoutingStats` / `GradientRoutingConfig` / `PathConfig`
  dictionaries, are gone from the UDL, the React Native TypeScript wrapper, both
  native modules and the Python bindings. `ProtocolConfig.path` is removed with
  them.

- **The bridge code that fed the table.** Both BLE managers learned a route on
  every completed inbound message and seeded one on every peer connect, and both
  ran a 30-second timer calling `cleanupExpiredRoutes`. All of it is removed.
  On Android that timer also swept stale inbound fragments, which is a live
  duty: the runnable survives as `fragmentSweepRunnable` and keeps that sweep on
  the same 30-second cadence.

### Documentation

- **Replicated documents have a guide, a state machine and examples that run.**
  The reference half was complete and the guide half was not: an application
  author could read every frame on the wire and still have nowhere to learn
  when a document is the right shape for their state.
  [docs/data.md](./docs/data.md) is that document, and it answers the questions
  the API reference cannot: message or document, what happens when two people
  edit one key, what each size limit does when it is reached, and what this
  version deliberately does not do.
  [The replication state machine](./docs/state-machines/data-replication.md)
  joins the other five with the local document states, the anti-entropy
  exchange and its triggers, the catch-up ladder, and the six ways an
  attachment fetch can end. Two programs make the behaviour executable rather
  than described: `cargo run -p offline-protocol --example replicated_notes`
  opens a store, edits all four collection types and reopens the same records
  after the engine is rebuilt, and
  `cargo run -p offline-protocol-data --example offline_merge` shows what two
  people editing one document offline actually get back.

- **The React Native guide no longer asks for a flag that is already on, and
  now lists the whole surface.** It still told applications to set
  `data: { enabled: true }`, the fifth restatement of a default that changed
  before it ever shipped. Its `DataStore` table was also missing every
  attachment method, the `DataValue` union it referred readers to, and its
  event-type list named none of the six `data_*` events, so an app author
  reading the platform guide could not tell they existed.

- **The native platform guides cover the data layer.** Both document group
  messaging with worked code and mentioned replicated documents nowhere, which
  left Swift and Kotlin readers with no path into the API their binding
  already exposes. Each now has a Replicated Documents section: the store,
  values as JSON, the durability rule, the bring-your-own-backend constructor
  with the logout obligation it carries, and the two events an application
  must handle.

- **The hand-written pieces and the routing table say so.** The TypeScript
  bridge contract records that the `DataValue` union is maintained by hand and
  names the Rust guard that pins it, because the one time it drifted both
  `cargo test` and `tsc` stayed green. CLAUDE.md's "read this first" table now
  routes a change in this area to the spec chapter, the state machine and the
  two ADRs, and the docs index calls the ADR set nineteen rather than fifteen.

- **`DataStore.wipeAll()` says that it is only durable once replication has
  stopped.** There are no deletion tombstones, so nothing distinguishes a
  space this device wiped from one it has never seen: called on a running
  engine with live sessions, the peer's next version offer recreates and
  refills every document, and an offer of our own naming nothing reads to the
  peer as a replica that has never seen the space. Every mention of the call
  framed it as the logout path, where the engine is being torn down anyway,
  and none said what happens anywhere else. Stated at the API reference, the
  upgrade guide, the configuration guide, the React Native and Python bridge
  docs, the bridge contracts, the storage adapter references and the method's
  own documentation.

- **`docs/mesh.md` no longer documents the dormant layer as the delivery path.**
  The "Path Scoring" section and everything under it (route entries, the
  routing-table configuration table, the UniFFI method table and the Swift and
  Kotlin route-learning examples) described machinery that never carried a
  frame. It is replaced by how a forwarding device actually chooses: live link
  state among addressable neighbors, with DORS choosing the carrier and the
  per-recipient facts applied at the send and acknowledgement seams.

- **Stale references corrected.** `docs/architecture.md`, the router crate
  README, `docs/api-reference.md`, `docs/configuration.md`,
  `docs/react-native-integration.md` and the React Native README no longer list
  the removed types. Two code comments cited `RelayManager::current_role()`, a
  method that stopped existing when relay standing moved to the forwarding
  governor. The DORS tie-break comments claimed an order ending at Reticulum
  when Nostr, not Reticulum, is lowest.

- **The gateway contract is specified** in
  [docs/spec/gateway-contract.md](./docs/spec/gateway-contract.md): what a
  gateway is, the five verbs it must implement (Attach, Submit, Verdict,
  Presence, Capabilities), the newline-JSON wire protocol a device speaks to a
  gateway daemon, and how the Reticulum backbone carries traffic between zones.

  The framing is deliberately a reframing rather than an invention: **the
  internet relay already implements every verb**, so everything the SDK does
  with the relay's answers today (parking, escalating probes, mesh offers,
  presence-driven flush) is gateway machinery that predates the name. Verdict is
  the load-bearing verb, and the contract states plainly that verdicts are
  unauthenticated claims which may open a path and must never close one, because
  nothing settles a message except the recipient's own acknowledgement.

  The device-to-daemon protocol promotes the one both mobile bridges already
  speak, extended with the verbs that make a gateway a gateway: a version field,
  an address declaration under a new `offline-gateway-addr-v1` signing domain
  (registered in the spec's signing-domain table as reserved), per-recipient
  verdicts, presence and capability advertisement. Attach is specified from both
  sides: the gateway verifies the proof, the address against the declared key,
  and its own single-use challenge, while the device verifies the address it is
  bound to. Neither check substitutes for the other, and a gateway that skips
  its half attaches sessions under addresses the attaching device does not
  control. Inbound delivery is specified alongside the five verbs (without being
  a sixth), because the daemon link is the only inbound path a device attached
  over local IP has, and a daemon built to the verbs alone would deliver nothing
  to the devices attached to it.

- **The reserved signing domain is pinned by the guard that owns that
  property.** `offline-gateway-addr-v1` joins the non-prefixing and distinctness
  tests in `protocol::types::signing_domain_tests` even though nothing emits it
  yet. Non-prefixing is a property of the whole set, so a guard watching only
  live domains would accept a future live domain that prefixes this one, and the
  collision would surface in whichever implementation first signed under it.

- **Two decisions recorded.**
  [ADR 0016](./docs/adr/0016-gateways-are-provisioned-not-emergent.md): gateways
  are provisioned, never emergent — the forwarding-bias shape does not transfer
  to bridging, because most devices structurally cannot bridge and there is no
  in-zone redundancy to cover a bridge that leaves, so self-promotion produces
  silent partitions.
  [ADR 0017](./docs/adr/0017-nostr-is-a-carrier-not-a-gateway.md): Nostr is a
  carrier and never a gateway, because a broadcast relay reports no
  per-recipient delivery. The resulting gap is recorded as permanent so it stops
  being rediscovered as a bug, and so nobody "fixes" it by inferring verdicts
  from evidence that is really about the relay.

- **Threat model grows a gateway section.** New adversary class A7 (hostile
  gateway) and residual R10, covering blackholing, verdicts that lie in either
  direction, zone-membership exposure to the operator, and backbone exhaustion —
  with the reason each is bounded to latency rather than loss.

- **`docs/reticulum.md` stops describing a daemon that does not exist.** The
  "Daemon TCP Protocol" section implied `rnsd` speaks this protocol; it does
  not, and no counterpart has ever existed in any repository. It now points at
  the contract spec and says so. Also corrected: "fourth transport" predated
  Nostr; the reconnection table presented inert Rust `ReticulumConfig` fields as
  live behaviour when reconnection is entirely owned by the native managers
  (1s doubling to 30s, not configurable) and configured from app config; and
  `RETICULUM_MAX_PAYLOAD_SIZE` is documented as a limit but has no reader.
  `docs/transport-architecture.md` aligned to match. The daemon section now
  also records what the shipped clients do *not* do: they confirm a send on the
  socket write and ignore `MessageSent` and `DeliveryError` entirely, so a
  daemon answering the contract correctly gets no verdict handling from today's
  bridges.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.22.x](docs/changelog/0.22.md) | 0.22.0 |
| [0.21.x](docs/changelog/0.21.md) | 0.21.0 |
| [0.20.x](docs/changelog/0.20.md) | 0.20.1, 0.20.0 |
| [0.19.x](docs/changelog/0.19.md) | 0.19.0 |
| [0.18.x](docs/changelog/0.18.md) | 0.18.3, 0.18.2, 0.18.1, 0.18.0 |
| [0.17.x](docs/changelog/0.17.md) | 0.17.0 |
| [0.16.x](docs/changelog/0.16.md) | 0.16.6, 0.16.5, 0.16.4, 0.16.3, 0.16.2, 0.16.1, 0.16.0 |
| [0.15.x](docs/changelog/0.15.md) | 0.15.0 |
| [0.14.x](docs/changelog/0.14.md) | 0.14.0 |
| [0.13.x](docs/changelog/0.13.md) | 0.13.1, 0.13.0 |
| [0.12.x](docs/changelog/0.12.md) | 0.12.0 |
| [0.11.x](docs/changelog/0.11.md) | 0.11.1, 0.11.0 |
| [0.10.x](docs/changelog/0.10.md) | 0.10.0 |
| [0.9.x](docs/changelog/0.9.md) | 0.9.4, 0.9.3, 0.9.2, 0.9.1, 0.9.0 |
| [0.8.x](docs/changelog/0.8.md) | 0.8.0 |
