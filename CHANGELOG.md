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

- **The identity assertion has one implementation.** The
  `public_key(32) || signature(64) || signed_data` value a Bluetooth LE
  peripheral serves to prove its address was split three times, once per
  bridge, and the Python copy was a JSON document no phone could verify. The
  codec now lives in `offline-protocol-sealed`, pinned by a generated vector
  file whose signatures are RFC 8032's own, and the one verifier is the
  namespace-level `verify_identity_assertion(assertion)`: it parses, verifies
  the signature under the key, derives the address, and returns it, or throws.
  A peripheral builds its value with `identity_assertion(signedData)` on the
  instance, so no bridge assembles the layout. The Swift and Kotlin decoders
  still split the bytes (they already called the core to verify and derive)
  and are now pinned to the sealed offsets by a source guard. This is the
  first piece of the peer-stream transport, which sends the same assertion as
  its first frame.

- **The peer-stream framing chapter.** `docs/spec/stream-framing.md`
  specifies the transport behind the `wifi_direct` slot as what it is to the
  engine: a stream the platform established to one peer, whether a Wi-Fi
  Direct group socket, a Multipeer session, or a TCP connection over a LAN or
  a routed mesh. The first frame in each direction is the identity assertion,
  every frame is a `u32` big-endian length plus body under the 1 MiB message
  ceiling, and a peer is announced only under the address its preamble
  proved. It also names the DNS-SD service type and TXT record a LAN
  advertiser uses, with the address there a hint the preamble proves. The
  vectors are generated: a preamble, a framed message, the ordered exchange,
  a prefix of exactly the ceiling that must be accepted, and four refusals.
  The chapter also records what the bundled managers still owe: iOS sends
  bare messages over its Multipeer session and advertises another service
  type, and the Android reader refuses a body of exactly 1 MiB and keeps
  reading after a refused length. A receiver holds one announced stream per
  address, so a second stream proving the same address never makes the first
  one's close read as the peer's loss. The threat model gains R16, the
  replayable identity assertion on every carrier. Nothing ships in this entry but the
  contract; the registration and the managers follow it.

- **The peer-stream transport is registered.** The transport behind the
  `wifi_direct` slot existed and was never constructed, so the slot's five
  FFI operations had nothing behind them: a body handed to
  `wifi_direct_message_received` was dropped before the engine saw it and
  `wifi_direct_get_next_message` was always empty. It is now registered
  whenever `wifi_direct_enabled` is set, rebuilt against the derived address
  like the others, and its transport callback is retained across that rebuild
  as the BLE one is. `docs/spec/stream-framing.md` is the contract, and the
  transport crate gains its Rust reference: `PeerStreamReader` reads a
  stream's bytes into events (the announced address, each message body, the
  close and why) for a host that owns its own sockets, and
  `stream_framing_vectors.rs` is the consumer that checks it against the
  chapter's vectors with the one real verifier behind it.

  Registering the slot changed two behaviours, deliberately. The slot counts
  as an available carrier only while a stream has proved a peer: with the
  platform's stream layer up and nothing proved it reports `connecting`, so
  the selector does not score it, a media transfer is not pinned to it, and
  the welcome lifecycle does not count it as a carrier. And the transport
  queues a message only toward an address a stream has proved, refusing any
  other recipient as not reachable on this carrier, which the selector treats
  as "try the next one". The bundled mobile managers report the layer up when
  they start and exchange no preamble yet, so they announce no peer. Without
  the narrower status, a file sent to a Bluetooth LE neighbour after the
  internet dropped was pinned to the slot, every chunk was refused, and the
  refused chunks reached the neighbour through the mesh with no window;
  without the refusal, the slot would win the selection on bandwidth and hold
  direct messages in a queue those managers cannot deliver from. With both, a
  phone with the slot enabled behaves as before this entry, and only a host
  that runs the framing carries traffic on it. A media transfer is pinned to
  the slot only when a stream has proved the recipient itself. The five React Native
  operations are no longer marked deprecated; their contract is the chapter's,
  and only a verifier's result may be announced.

- **A Python host can reach another over a LAN or a routed mesh.**
  `PeerStreamManager` is the peer-stream transport for a host that owns its
  sockets: a TCP listener, outbound connections to configured `host:port`
  peers (or `off1…@host:port`, which makes the derived address have to match
  the claim exactly) and to hosts found over DNS-SD, and the preamble exchange
  the chapter specifies. Each side sends its identity assertion as the first
  frame without waiting for the other's, verifies what it receives through
  `verify_identity_assertion`, and announces the peer under the derived
  address only; a stream that fails any step is closed and was never
  announced. `ProtocolManager` builds it when `wifi_direct_enabled` is set
  and stops it with the rest; the application starts it after
  `ProtocolManager.start()`, since a host with no address to prove is refused
  by every peer. DNS-SD advertises `_offlineprotocol._tcp` with `txtvers=1`
  and `addr=<off1…>` through the optional `lan` extra
  (`pip install 'offline-protocol-sdk[lan]'`), and the `addr` in a record is
  a hint the preamble proves, never a name to announce. Two hosts that list
  each other keep one stream, the one the lower address opened, so a
  simultaneous open cannot loop; the lower address reconnecting while its
  old stream is still half-open supersedes it. The higher address cannot, so
  every stream carries TCP keepalive and a dead peer's stream ends within
  about half a minute. The preamble deadline covers the whole first frame,
  one remote host holds at most eight inbound streams, and a peer that never
  proves an address is retried on a doubling ladder. The listener binds
  every interface by default; pass `listen_host` to narrow it.

- **Attachment bytes move inside a group.** `DataStore.fetchAttachmentFrom(space, peer, hash)`
  asks one member for the bytes behind a reference, and they come back as
  frames sealed under the group key. Before this, references replicated to
  every member and the bytes reached nobody: fetching them needed a confirmed
  pairwise session, and two members of a group need not have one with each
  other.

  The fetch names the member for two reasons. A reference says what the bytes
  are and nothing about who holds them, so somebody has to choose; and a
  directed frame is promoted to a roster-wide delivery when the sender's
  ratchet budget runs out, which means a request arrives at every member
  whether or not it was addressed to them. Without the name in the frame,
  every member's application would be asked for bytes the requester can
  accept from only one of them. The SDK does not try members in turn, because
  each miss costs the whole silence timeout.

  **A group attachment is at most 1 MiB**, 32 frames of 32 KiB. That bound is
  what keeps the request a single question with a single answer: the whole
  answer leaves at once, so nothing has to ask for the next window and there
  is no chain of questions to terminate. `provideAttachment` refuses a larger
  blob at the call, while the application still has the file in hand, and
  says to use a 1:1 session instead. Nothing about the 1:1 path changed; it
  still carries up to the transfer layer's own limit.

  A chunk is admitted only against an outstanding request and only from the
  member that request was put to, and the assembled bytes are still checked
  against the hash that asked for them. The first two bound what a member can
  spend of another's memory and stop one member answering or refusing a
  question put to somebody else; only the hash says what the bytes are.

  Negotiated as `data_versions` entry 6, appended, and the first entry since
  the second to have an attested sibling: a group inviter now attests every
  group-relevant entry it knows about a member, rather than only entry 2. A
  member without entry 6 refuses the request it understands and drops the
  chunk frames it does not, so nothing is shown to anybody wrongly; what the
  entry buys is refusing a fetch at the call instead of waiting out the
  timeout.

  A fetch is refused at the call for a member this device does not know
  receives replication frames inside a group, because one that does not
  would be shown the question as text, and for this device itself. Asking a
  second member is a fallback rather than a repeat: the question put to the
  first is reported as `evicted`, naming that member, and the new one goes
  out at once. Every report of a fetch that ended without bytes now names
  the member it was put to rather than the space.

- **A space can replicate in part.** `DataStore.setInterest(space, patterns)`
  names the documents this device wants from its peers: document names, each
  optionally ending in `*` to match a prefix, at most 32 per space. `["*"]`
  is everything and is the default, so a space nobody narrows behaves exactly
  as before and its frames are byte-identical to the ones it sent last
  release.

  It does two things that fail differently, and the difference is the whole
  design. Toward a peer it is a **request**: every version offer carries it,
  so the peer answers only with what was asked for. That is the bandwidth
  saving, and it needs the peer to read the field. Locally it is a
  **refusal**: a document outside the patterns is never created from an offer
  and never imported however it arrives. That half depends on nothing, which
  is why a narrowed space is narrow even against a peer that ignores the
  request, including every build that predates it.

  Three rules keep it from losing documents. The answer is scoped by the
  asker's declared interest and never by the names their frame happened to
  carry, because the second drops a document they have never seen with no
  symptom on either device. A counter-offer still carries the complete list:
  the saving there is one name per unwanted document, which is not worth
  narrowing a reply sweep for. And removals are never scoped, because a peer
  that narrowed still holds what it held before, so a removal for a document
  it no longer wants is exactly the one it still needs.

  Interest is not persisted: it is application policy for this launch rather
  than a fact about the store, so declare it before `start()`. `wipeAll()`
  clears it along with the documents it scoped, because nothing may
  distinguish a wiped space from one this device has never seen. Narrowing
  does not delete what is already held, because a policy change should not
  destroy data nobody asked to lose; `deleteDoc` is how a document leaves.
  Widening asks, since the newly wanted documents are absent from this
  device's next offer and inside its declared interest.

  Negotiated as `data_versions` entry 5, appended. On the wire an absent
  `want` means everything and an empty one means nothing; the two are
  deliberately not collapsed, because collapsing them would turn the
  narrowest request into the widest.

- **Replicated documents can be removed from every replica.**
  `DataStore.removeDoc(space, doc)` records the version the document stood at
  and tells the space; a replica whose copy that version covers deletes it too
  and reports the new `data_doc_removed` event with `by: "peer"`.
  `removeSpace(space)` does the same for every document a space holds. A
  removed name refuses reads and writes with `InvalidState` until `createDoc`
  brings it back. Before this, a document deleted on one device was absent
  from its next offer and still present on the peer's, and the rules that
  exist to carry a document a peer has never seen recreated and refilled it:
  a delete was undone by the next exchange, with no error and no event.

  The removal is a version rather than a flag, which is what lets every
  replica reach the same answer without knowing what happened first. Content
  at or below it is what the removal decided about; anything beyond it is an
  edit made concurrently with the removal, and **an edit wins**. It brings the
  whole document back, not the edit alone, because the edit was made on the
  old contents and its history is those contents. That arrives as an ordinary
  `data_changed` on a document the application removed. A product that needs a
  removal which cannot be undone that way should model it as a field it
  writes rather than as a document that disappears. For the same reason a
  removed name is not fenced off when it is re-used: a replica that kept the
  old contents past the removal merges them into the new document, and a
  replica on an older build merges the new contents into its old copy. Give
  new content a fresh name.

  A removal never expires, so a device away for a year still learns about it,
  and a removed name keeps a small record for the life of the space. Those
  records count against the existing 1024-document ceiling a peer can fill,
  because a floor outlives its document and counting only what is held would
  let a peer name a fresh thousand after every removal. The record is written
  before the records it removes: the reverse order leaves, after a crash in
  between, records whose presence votes the document back into existence at
  the next open.

  `deleteDoc` is unchanged and is eviction: it drops this device's copy and
  records nothing, so a replica that still holds the document refills it.
  `wipeAll()` is unchanged too and records no removals, because a logout has
  to leave a custom backend empty. The four documents that described the old
  behaviour say so.

  Negotiated as `data_versions` entry 4, appended. A peer without it ignores
  the new field, keeps its copy, and offers it back on every exchange; the
  removing side refuses each offer and each blob against its own record, so
  the removal holds there regardless. Nothing loops and nothing is surfaced
  wrongly. New storage category `data_doc_meta`, sealed like every other, and
  no new error code. `listSpaces()` includes a space whose documents were all
  removed, because it still has removals to report.

### Changed

- **`WIFI_DIRECT_MAX_PAYLOAD_SIZE` is gone from the transport crate.** It
  stated a 64 KiB ceiling nothing read, and the peer-stream chapter's ceiling
  is the 1 MiB message ceiling; the framing constants that replace it are
  `PEER_STREAM_MAX_FRAME_BYTES`, `PEER_STREAM_LENGTH_PREFIX_LEN` and
  `PEER_STREAM_PREAMBLE_FLOOR`.

- **The telemetry pipe caps `protocol.message.failed` rows per reason.** Each
  reason sends up to 500 rows at once, enough for a full outbox failing in one
  sweep, and then one row every ten seconds. Every failure is still counted
  into the minute rollup's `sends_failed_sum`, so the totals are exact; only
  the per-row `reason` and `retry_count` are thinned, and a held-back row is
  counted in `telemetryStats().dropped`, so sent plus dropped still accounts
  for every event. A row cannot carry a message id, so the ingest has no way
  to tell one failure reported many times from many failures and meters every
  row. Without the cap, a device reporting the same failure in a loop is billed
  for the loop, which is what an outbox bug fixed in this release did at about
  fifteen rows a second per device. The check runs on the borrowed event before
  anything is copied, so a held-back row allocates nothing. There is no
  setting.
- **A frame the relay's mailbox holds is parked without a reachability probe.**
  A relay advertising `mailbox_v1` keeps a copy of a frame it could not deliver
  and re-sends it on the recipient's next connection, and says so with
  `stored: true` beside the `DeliveryError` or the pushed `MessageSent`. The
  React Native bridges and the Python relay client now read that flag and
  report those two answers to the core as the new `relay_stored` and
  `relay_pushed_stored` tokens. The park they produce is the existing one in
  every respect that matters - the pending acknowledgement is dropped so no
  budget burns against an offline peer, the outbox entry stays, the frame is
  still offered to the mesh, and the recipient is still presence-watched - and
  drops only the escalating 15s→600s probe. The probe existed because nothing
  else would re-deliver the frame; now something does, and each rung would only
  rewrite the row the relay already holds, per parked message, for as long as
  the peer stays away.

  Recovery is the relay's own redelivery, and the reachability edge behind it. A
  returning peer already re-drives every parked frame the outbox holds, so a
  held copy the mailbox evicted or expired is covered by the same edge that has
  always covered an unheld one - which means what the token saves is the probe
  rungs while the peer is away, not the write on its return. Past the mailbox's
  own retention the guarantee is that edge and the outbox lifetime: the trade
  `edge_driven_unreachable_dm` opts into for every parked frame, taken here for
  held frames alone, where something else is already committed to delivering
  them.

  Everything that is not a plain direct message is deliberately untouched: both
  tokens normalize back onto the existing vocabulary before any typed branch
  reads them, so a connection request still fast-fails with
  `ConnectionRequestUndeliverable` and a Welcome still parks on its own
  lifecycle. `MessageUndeliverable` is still emitted on the `DeliveryError`
  path, because it is documented as a repeatable status signal and "the
  recipient is offline right now" is true whether or not a copy was kept. A
  missing `stored` key - an older relay, a relay whose mailbox is off, a store
  that failed - reads as "not held" and keeps the probe.

  `stored` is a statement about one message, not about the recipient. The
  `DeliveryError` path fails every frame in flight to that recipient, so only
  the id the relay echoed takes the new token and the rest keep the
  `recipient_unreachable` prefix they have always had. Each held frame takes it
  from its own verdict: the relay answers every submission separately, and the
  first answer for an offline recipient resolves that recipient's whole
  in-flight set, so a client reports the id its verdict named even when that set
  is already drained. Without that, only the first frame of a burst would park
  probe-free and the rest would each pay one probe before the relay said
  `stored` again. A Welcome reported that way has usually been failed already
  by the drain, and is not failed a second time: one send climbs one rung of its
  retry ladder, and the app hears `welcome_send_failed` once.

### Fixed

- **The mobile bridges map every transport name.** `forceTransport` and
  `getTransportMetrics` on iOS and Android each carried a three-name copy of
  the transport-name mapping that defaulted anything else to BLE, so forcing
  `"nostr"` or `"reticulum"` from JavaScript forced the radio, and a metrics
  read for either answered for it. Each bridge now has one mapper that names
  all five transports and refuses an unknown name, both entry points go
  through it, and a Rust guard reads both sources to keep it so.

- **A send that every carrier refused as not reachable says so.** When the
  selected carrier refused the recipient and there was nothing left to fall
  back to, the send ended as `SendFailed("All transports failed")`, and the
  `message_deferred` reason read `transport_send_failed`, as if a carrier had
  broken mid-send. It now ends as the refusal, and the reason is
  `peer_not_reachable`. The welcome lifecycle classifies both the same way,
  so only the deferral reason changes.

- **A Python Bluetooth LE peripheral can be verified by a phone.** It served
  the profile label as its Device id and a JSON document as its Identity, so
  every conforming central refused it before any cryptography ran; no phone
  has ever paired with a Python node over Bluetooth LE. The peripheral now
  serves its `off1...` address and the core-built identity assertion, and
  `start()` refuses to advertise until MLS has minted an address rather than
  advertise something unverifiable. The `identity_json` constructor argument
  is gone. Any Python peripheral already deployed changes behaviour, but no
  working pairing regresses, because none could have worked.

- **The Python Bluetooth LE central verifies a peer before announcing it.** It
  announced whatever string the Device id characteristic carried, and before
  connecting it announced the Bluetooth address itself, so the core's routing
  table held labels nothing had proved. It now reads the Identity
  characteristic, hands it to `verify_identity_assertion`, compares the result
  to the Device id exactly, and announces the derived address or drops the
  link unannounced. A link that never proved a peer is no longer reported lost
  either, since nothing was ever announced for it.

- **Relay group notifications reach an iOS or Android member.** The React
  Native bridges rebuild some relay traffic into a full `Message` before
  handing it to the core: the relay's answers about groups (`GroupCreated`, a
  relay-broadcast `GroupMessageReceived`, member added and removed, group
  errors, group info and the user's group list) and plain-text messages from
  pre-SDK relay senders. They addressed every one of those frames to the
  profile name. Since the release that derived the wire identity from the
  identity key, a device's id is its address once `initialize_mls` has run,
  and the core forwards rather than processes a frame addressed to anyone
  else. So on every phone with encryption on, all of them were handed to the
  mesh forwarder and lost silently.

  The loss was hidden rather than harmless. `GroupCreated` is the relay's
  registration acknowledgement and the only thing that lets a group use the
  relay's one-frame broadcast, so phones never took that path and fell back to
  sending one copy per member, which still arrived. A group message another
  member broadcast through the relay never arrived on a phone, and a relay
  group error never revoked the registration. A phone with encryption off was
  unaffected.

  The frame builder on both platforms now takes the device's address and its
  profile and addresses the frame to the address, falling back to the profile
  only before initialization, which keeps those frames byte-identical. No wire
  format changed.
- **A group message broadcast through the relay reaches a Python member.** A
  phone whose group is registered with the relay sends one frame, the relay
  fans it out to every member as `GroupMessageReceived`, and counts the socket
  write as delivered, so the sender never re-sends a per-member copy. The
  Python bridge re-injected that frame under a prefix the core does not
  recognize (`__GRP_MSG__`), addressed to the profile name rather than the
  device's address, and without the `id` and `timestamp` the deserializer
  requires. The core treated it as plaintext for someone else and the message
  was lost silently. The bridge now injects it as the iOS and Android bridges
  do: `__GROUP_MSG__`, addressed to `local_address()`, in the full `Message`
  shape. `GroupCreated`, `GroupMemberAdded`, `GroupMemberRemoved` and
  `GroupError` are injected unattributed under the core's relay-answer
  prefixes, and `GroupMessageSent` goes to `internet_group_report_received`
  instead of the message plane. The same frame builder now backs the
  plain-text `MessageReceived` fallback, which was dropped for the same
  reason.
- **A message evicted from a full outbox stays failed.** Eviction removed the
  outbox entry and reported `message_failed` (`"Outbox capacity exceeded"`),
  but left the message queued for retry. The next retry, or the next reconnect
  flush, re-created its outbox entry with fresh timestamps and evicted a
  different message to make room, whose own retry did the same. A device
  holding more than 500 undeliverable messages therefore reported a terminal
  `message_failed` on every retry, indefinitely, for ids it had already
  reported. Nothing ever ended the cycle, because the outbox is the only thing
  that expires a message and each resurrection reset its clocks. With the
  telemetry pipe on, every one of those reports was an uploaded, metered
  event. The media outbox leaked retry entries the same way, silently.

  Every path that gives a message up (capacity eviction, lifetime expiry, retry
  exhaustion, an acknowledgement timeout that finds no outbox entry) now takes
  it out of the retry queue, the acknowledgement tracker and the outbox
  together. An application that counted `message_failed` events saw that count
  inflated and will see it drop to one per failed message.
- **A message evicted while awaiting its acknowledgement is settled once.** Its
  acknowledgement stayed tracked, so the timeout reported it failed a second
  time (`"Message missing from outbox (cannot retry)"`), and an acknowledgement
  arriving first reported it delivered after it had been reported failed.
- **Bluetooth pairing works on a phone running several apps built on this
  SDK.** Every such app serves the same BLE service, so a phone running two of
  them presents two instances of it behind one connection. The React Native
  bridges assumed one. On iOS the handshake read both instances into one slot
  and joined whichever reads landed first, so the peer was refused or announced
  under the other app's identity at random; on Android it always bound to the
  first app to register. Two copies of one app could stop finding each other
  as soon as a second SDK app was installed on either phone.

  Each app now serves an eight-byte tag derived from its `appId` in a new,
  optional GATT characteristic (`6E400005-…`), and a central that finds several
  instances reads their tags and binds to its own app's, then handshakes,
  subscribes and writes on that instance only. A phone running only other SDK
  apps is still bound (to the first instance, as before) and still relays.
  A peer running one SDK app is handled exactly as before and its tag is never
  read. No API or configuration changes; the tag comes from the `appId` the app
  already passes. An app built on an earlier SDK serves no tag, and one such
  build among tagged apps is still found; it cannot find its own counterpart on
  a multi-app phone until it upgrades. See
  [BLE framing](docs/spec/ble-framing.md#several-instances-behind-one-link).

## [0.26.0] — 2026-09-12

> **The SDK ships its own telemetry.** Until now the SDK handed every telemetry
> record to application code through a sink callback, and a separate React
> Native package classified, batched and uploaded them. That package was never
> published, Python and native hosts had no upload path at all, and every
> record crossed the bridge twice. The whole pipeline now lives in the core:
> `enableTelemetry({apiKey, appId, appVersion})` and the SDK collects, folds,
> queues and uploads accepted events to the ingest on its own background
> thread. Nothing reaches JavaScript per event, the emit path does less work
> than the 0.25 sink path, and a device with telemetry off pays what it paid
> before. **Breaking on every binding:** the sink API is gone and
> `TelemetryConfig` is reshaped; [§20](docs/UPGRADING.md#20-the-sdk-uploads-its-own-telemetry-v0260)
> is the migration table.
>
> **The Rust crates now open one socket.** The README's "never open a socket"
> claim held until this release; it is now true of the protocol and false of
> the telemetry pipe, which posts to one endpoint fixed at build time over TLS
> with a bundled root set. The threat model gains a network-egress section and
> two residual risks for it.
>
> **Delivery state survives a restart, and two defaults grow.** The inbound
> pending-decryption queue and the deduplicator's seen set are now persisted,
> sealed like the outbox, so a restart during a slow handshake no longer loses
> the frames parked behind it, and a message that reaches the device twice is
> still one message across the restart. To match, `pendingQueue.pendingTtlMs`
> defaults to 24 hours (was 30 minutes) and `reliability.dedup` to 2000 ids
> over 24 hours (was 1000 over 1 hour) wherever a binding fills them in. A
> direct message the
> relay could only push is parked rather than left awaiting an ACK, with no
> `MessageUndeliverable` for it, and the React Native bridges hold inbound
> message events while nothing can take them. None of this changes a
> signature;
> [§21](docs/UPGRADING.md#21-delivery-state-survives-a-restart-and-two-defaults-grow-v0260)
> says what each looks like from the application.

### Added

- **The inbound pending-decryption queue is persisted.** A frame that arrives
  before its sender's MLS session is ready is written to protocol-state storage
  (`pending_decrypt_entries`, sealed like the outbound pending queue) when it
  is admitted, deleted when it is drained, evicted or discarded (blocking the
  sender discards it), and restored on the next launch. A restart during a slow handshake used to lose it, and a
  relay that pushes ciphertext without store-and-forward holds no second copy
  to ask for. A record older than seven days on disk is dropped on restore and
  reported as `PENDING_QUEUE_DROPPED` with reason `expired_persisted`.
- **The deduplicator's seen set is persisted.** Up to 2000 inbound ids, newest
  first, sealed like the pending queues, written in batches (32 changes or 5
  seconds on the process tick,
  flushed on stop) and restored beside the Lamport clock, so the relay-socket
  copy of a message the app already consumed from a push injection is
  recognised as a duplicate across a restart instead of reaching a spent
  ratchet generation and surfacing as a decryption failure. A runtime
  `updateDedupConfig` carries the set across instead of clearing it. A
  sender's own outgoing ids are tracked for the process only and never
  written.
- **`PENDING_QUEUE_DROPPED` is emitted for text messages**, not only for media
  chunks, whenever the pending-decryption queue evicts a frame (overflow, TTL,
  or the persisted age bound). Text evictions were metrics-only, which left an
  app unable to tell "the sender went quiet" from "the SDK evicted their
  message". The event is advisory: the frame was never ACKed, so a sender
  still retrying resends it.
- **Inbound message events are held while nothing can take them.**
  `message_received`, `file_received` and `message_decryption_failed` each
  report one message the core has already ACKed and will never restate, so a
  drop between the core and the app's handler was a lost message. Each React
  Native bridge now holds up to 256 of them, keyed by message id, while
  JavaScript has no listener or no live React instance (a backgrounded app, a
  push injection before the first subscribe), and replays them on subscribe
  and on foreground; the TypeScript layer does the same across the gap to the
  app's first `on()`. Held events are scoped to the session that produced
  them, so a message held for a torn-down account never surfaces in the next
  one.
- **`enableTelemetry` / `enable_telemetry` on every binding.** Takes the API key
  and app id the developer portal issued; the binding fills in the platform
  and owns the application lifecycle, so there is no per-event or per-lifecycle
  call for the app to make. `disableTelemetry`, `flushTelemetry`,
  `telemetryStats`, `endTelemetrySession` and `setTelemetryEnabled` complete
  the surface; the Rust and Python surfaces add `notify_app_state` for hosts
  with a lifecycle of their own. `telemetryStats` reports `sentEvents` and
  `acceptedEvents` separately, the latter being what the service meters, so an
  invoice can be reconciled against the device.
- **The pipe itself** (`offline_protocol::telemetry::pipe`): classification by
  name with no allocation for the sixty-odd events it drops, the 60 s metrics
  rollup, the session summary with delta counters, a session-stamped ring
  buffer, a durable batch queue on the protocol-state storage seam (sealed
  records, one per batch, bounded at 64 batches and 4 MiB), and an HTTPS
  uploader that follows no redirect (a 3xx drops the batch) and can use an HTTP
  CONNECT proxy the environment names, with idempotent retry: 1 s doubling to
  15 min with jitter, `Retry-After` honoured, a six-day expiry inside the
  ingest's deduplication window, a halt on 401 and 403, deferral below 15%
  battery unless charging, and one final flush of up to three seconds on disable
  and on end of session. A background transition wakes the uploader without
  blocking; iOS additionally wraps that flush in an OS background task, which
  Android has no equivalent for. The transport-availability edge the tick
  already computes wakes it when the internet or Nostr transport comes back.
- **`offline-protocol-telemetry-wire`**, a new crate holding the batch envelope
  and typed event payloads shared with the ingest, with the canonical wire
  fixture pinned byte for byte and a second fixture pinning the additive raw
  `reason` field.
- **A golden test against the frozen TypeScript client**: the same scenario
  replayed through both, compared batch by batch, with the four documented
  divergences (raw `reason`, the SDK version, and the two summary fields
  below) normalized.

Two summary fields deliberately do not match the client this replaces, because
the client's answers were wrong on a device rather than merely different:

- `mls_session_ready_latency_p50_ms` paired `mls.initialized` with
  `mls.session_ready` on the MLS session id. That id is derived from
  `peer=<id>|group=<id>` and the group is minted during the handshake, while
  `mls.initialized` is emitted once per process for no peer at all, so the
  pair never formed and the column was always absent. The pipe pairs a
  handshake start with `mls.session_ready` on the peer instead, which
  measures the time from wanting a session to having one. A new
  `mls.session_establishing` lifecycle event marks that start, emitted where
  a session is created from a peer's key package; it never reaches the wire.
  An outbound `mls.session_missing` opens the window too, for the send that
  found no key package either and had to wait for one, and the earlier of the
  two wins. Inbound misses do not: a frame that outran its Welcome would
  otherwise contribute Welcome reordering to a handshake this device never
  started.
- `session_duration_s` was reported from the session start while every other
  field is a delta, so a session that produced two summaries had its seconds
  counted twice. It is now the span since the previous summary, and summing a
  session's rows gives its length once. A boundary that repeats a summary
  with nothing observed since no longer emits a row at all. This is a wire
  contract change and not a restored one: the ingest stores the column and
  reads it in neither plane today, so nothing downstream breaks, but a reader
  added later must treat rows written before this release as totals rather
  than deltas.
- **A debug tap.** `debug: true` logs every wire event on the platform log
  (logcat, the unified log) or stderr, which needed a place for the pipe's
  `tracing` output to go on a device; the uniffi crate now installs one. It
  carries the pipe's own target and nothing else: the engine's internal
  logging names groups, senders and peers, and a device log is readable by
  anything that can run `logcat`, so the filter is a privacy boundary rather
  than a volume setting. An embedder that wants the whole stream installs its
  own subscriber, which the SDK leaves in place.
- **An Apple privacy manifest** shipped with the pod, and
  [docs/privacy.md](docs/privacy.md) with the store disclosures.
- `ProtocolError.TelemetryConfigInvalid`, appended at position 24, naming the
  refused field.

### Changed

- **A DM the relay only pushed is parked, not left awaiting an ACK.** The relay
  answers `MessageSent { pushed: true }` when the recipient has no live socket
  and the ciphertext went out in a device push; it keeps no copy. The React
  Native bridges and the Python relay client now report that id to the core as
  `relay_pushed`, which drops
  the pending ACK, keeps the outbox entry, schedules the reachability probe
  and presence-watches the recipient, as a `DeliveryError` does, with two
  differences because the push may have delivered the frame: connection
  requests and Welcomes stay on their ordinary path, and no
  `MessageUndeliverable` is emitted, on the push itself or on any later probe
  verdict for that frame. The outbox entry remembers it was pushed (persisted
  with the entry), because the relay answers a probe of a pushed message with
  `DeliveryError` rather than a second notification and the core would
  otherwise read that as an ordinary unreachable verdict.
- **`pendingQueue.pendingTtlMs` defaults to 24 hours** (was 30 minutes) on
  every binding that fills the field in (the Python config record requires it,
  as before), so a frame parked for a session still being set up outlives
  a handshake on a phone opened once a day. The in-memory TTL restarts with
  the process; the persisted copy is bounded separately at seven days. Memory
  stays bounded by the per-peer and global caps.
- **`reliability.dedup` defaults to 2000 ids and 24 hours** (were 1000 and
  1 hour), sized for the persisted seen set. The Nostr replay overlap
  (1 h 5 min) now fits inside the retention window, so a reconnect the next
  day deduplicates it instead of re-processing it; `docs/nostr.md`, the
  transport's subscription doc and the drift guard now state the absorption.
  The React Native `updateDedupConfig` fallback for an omitted
  `maxTrackedMessages` is 2000 as well (was 10000), so a `dedup` section that
  sets only the retention no longer tracks five times the documented default.
- **`TelemetryConfig` is reshaped** on every binding: `apiKey` and `appId` are
  required, `appVersion`, `debug`, `flushIntervalMs`, `maxBatchBytes`,
  `maxBufferedRecords` and `includeDeviceId` are new, `enablePollQueue` is
  gone, and the five earlier knobs keep their names and defaults.
- **`reason` goes up raw.** The earlier client classified the engine's failure
  reasons into a fixed family on the device; the pipe sends the engine's own
  token or literal and the classification happens on the ingest. Every reason
  the engine produces is locally chosen, per the threat model's producer rule.
- **`scrubIds` never changes what the pipe uploads.** The pipe reads the peer id
  on MLS lifecycle records, hashed unless `scrubIds` is false, only to pair a
  handshake's start with its end. No identifier has a wire field to land in, and
  the golden fixture pins the uploaded bytes, so the option changes what the
  pairer holds in memory and never what is uploaded.
- **Where the mesh-analytics package differed from the task, the task won:**
  a 15 min backoff cap rather than 5, a 413 dropped rather than split, a halt
  on 401 and 403 rather than a drop-and-continue, `Retry-After` honoured on a
  429,
  `Idempotency-Key` and `User-Agent` sent, 15 s request and 10 s connect
  timeouts, and the battery deferral. Each is a named test.
- **A 401 or 403 keeps its batch.** The halt already stops every later send,
  so dropping the batch that drew the refusal lost data for nothing: a
  rejected key is a configuration fault the developer fixes and re-enables
  through, and the queue now waits for a good one. `telemetryStats().dropped`
  counts events and only events, so a queued batch that cannot be opened on
  load is warned about rather than added to it in a different unit.
- **A 403 `telemetry_disabled` drops the queue.** The ingest answers with that
  `error` code, rather than the `forbidden` a bad key draws, while the
  application's telemetry toggle is off in the developer portal. The pipe
  drops the refused batch and everything queued behind it, discards what it
  collects afterwards instead of queuing it, and sends nothing more until the
  next `enableTelemetry`, so a toggled-off stretch stays a gap on the
  dashboard rather than filling in from the device's queue once the toggle
  comes back. Every other 401 or 403 keeps its queue as above, and so does a
  403 from an ingest that predates the code, which answers `forbidden`.
- **Telemetry session boundaries follow the process on Android**, through the
  set of started activities rather than `onHostPause`. `onHostPause` is
  `Activity.onPause`, which fires for a runtime permission dialog (including
  the Bluetooth one the SDK triggers itself), the share sheet and any
  translucent activity, so it counted one iOS session as several on Android.
  A configuration change no longer closes a session either. Activities are
  tracked by identity rather than counted, because React Native builds its
  native modules after the host activity has started: a counter began at zero
  with an activity already on screen, so opening a second in-process activity
  (a sign-in hub, an image picker) drew a background edge with the app fully
  visible. The state seeded at `enableTelemetry` comes from the same watcher
  for the same reason, rather than from React Native's `lifecycleState`,
  which is also pause-granularity.
- **`setTelemetryEnabled(false)` stops the session boundaries too.** A summary
  is a row the ingest stores and bills, so emitting one per background for a
  user who switched telemetry off contradicted what that switch promises. The
  transitions are still tracked, so the first foreground after collection
  resumes rotates the session as before, and a backlog queued before the
  switch still drains.
- **Enabling telemetry in the background no longer reports an empty session.**
  The host's application state is recorded rather than replayed as an edge; a
  background launch (an iOS BLE restoration, an Android headless mesh wake)
  was otherwise billed one all-zero summary with a zero duration, and woke the
  uploader for it. The seed still arms the boundary, so the first foreground
  rotates.
- **A queue cut under one `appId` is discarded rather than uploaded under
  another.** The app id travels in a request header, not in the batch body, so
  a queue left behind by a previous `enableTelemetry` would have been counted
  against whichever key was configured next. The queue survives
  `disableTelemetry` by design, and is cleared by uninstalling, by
  `wipePersistedState`, and now by an app-id change.
- **Re-attaching the same protocol-state storage to the pipe keeps its queue.**
  Both `enableTelemetry` and `initializeMls` hand the backend over, so the
  ordinary logout-and-login sequence ran it twice; the second pass reloaded the
  pending batches and then re-persisted the in-memory copies, queueing every
  one of them twice and trimming the originals to the byte cap.
- **One pipe writes a telemetry queue at a time.** A pipe stopped mid-upload
  finishes on a detached thread, and the pipe that replaced it (a second
  `enableTelemetry`, or a new engine over the same storage after a teardown)
  used to adopt the queue at once. Both rewrite the index from their own
  memory, so the old thread's last write could leave the replacement's
  batches unnamed, and the next launch swept them as orphans without counting
  or logging them. The replacement now holds its batches in memory until the
  old thread lets go, then adopts the queue behind what that thread left, and
  a pipe that finds its queue owned by another pipe, or cleared, stops writing
  to it. An index entry whose record was already deleted before a crash is no
  longer logged as a discarded batch.
- **`enableTelemetry` and `disableTelemetry` no longer interleave.** A disable
  that landed inside an enable stopped the new pipe after the enable had
  installed it, and the flush, stats and lifecycle calls then ran against
  that stopped pipe. On Android the lifecycle watcher also reads the protocol
  handle as a volatile field, so a background edge cannot be skipped on a
  stale null.
- **`telemetryStats().lastError` clears when a batch is accepted**, so it
  reports the current state rather than the high-water mark of a recovered
  outage.
- **`Event::message_failed` and `Event::relay_demoted` take `&'static str`.**
  The telemetry pipe forwards these reasons to the ingest verbatim, without
  the scrubber, because the classification taxonomy is the ingest's. A
  `String` parameter made `format!("{err}")` representable at a call site, and
  the transport layer renders the counterparty into its own error text, so one
  such interpolation would have shipped a peer address beside identifiers the
  same record hashes. `Event::message_deferred` already took a static string;
  `WelcomeReasonCode::welcome_failure_reason()` is the static form of the one
  call site that was interpolating.
- The `data` and `telemetry-pipe` cargo features are both on by default; a
  build that wants neither the CRDT engine nor the TLS stack opts out.
- `LICENSE-COMMERCIAL.md` states the telemetry term every commercial license
  carries, as set out in the applicable commercial agreement. Hosted telemetry
  remains optional. Without written authorization, the bundled client may not be
  redirected to an unauthorized endpoint or another telemetry service, or have
  its event schema or serialized payload format changed. Ordinary forwarding
  proxies, local test captures, and independent observability built on public
  APIs remain permitted. The AGPL-3.0 option is unchanged.

### Removed

- **The telemetry sink surface**: `installTelemetrySink`, `onTelemetry`,
  `pollTelemetry`, `uninstallTelemetrySink` and the `TelemetryRecord`,
  `TelemetryListener`, `MetricsFrame`, `TransportStateTelemetryEvent`,
  `RoutingDecision`, `DeviceCapabilitySnapshot`, `TransportMetricsEntry`,
  `RetryQueueStatsFrame`, `DeduplicatorStatsFrame`, `RoutingScoreEntry`,
  `RoutingPhase`, `RoutingReasonCode`, `TransportStatus` and `RelayRole`
  TypeScript types; the UDL `TelemetrySink` callback interface and the
  dictionaries and enums that existed only to type it; the Python
  `install_telemetry_sink`, `uninstall_telemetry_sink` and
  `poll_telemetry_frame` methods. The Rust `install_telemetry_sink` is
  crate-private. The free event API (`onEvent`, `on_event`) is unchanged and
  stays per-event and unaggregated by design; the rollup and session summary
  exist only inside the pipe, and a test pins that neither is ever an event.

### Fixed

- **A peer-requested session reset deletes the persisted pending-decryption
  records** of the frames it discards. Left on disk, the next launch restored
  frames sealed to the deleted session and drained them into its replacement
  as spurious decrypt failures.
- **Releasing a dropped group entry's replay protection reaches the persisted
  seen set.** The release unmarked the id in memory only, so a record written
  while the entry was buffered kept the id, the next launch restored it, and
  the sender's redelivery was swallowed and re-ACKed as delivered for the whole
  retention window.

### Documentation

- **ADR 0024 records why the SDK ships its own TLS stack for telemetry** rather
  than handing the upload to a host HTTP callback: `ureq` with exactly its
  `rustls` feature and the bundled Mozilla roots, no redirect, an HTTP CONNECT
  proxy from the environment only, and each of those settings named in the
  uploader's agent configuration and pinned by a test, so a `ureq` upgrade
  cannot change them silently. An interception certificate installed in the
  device trust store is not trusted by the SDK.
- **`docs/telemetry.md` is rewritten around the pipe**: what leaves the device
  and when, sessions, durability, the stats call, the debug tap, controlling
  cost, keys and revocation, and identifier scrubbing. `docs/privacy.md` sits
  beside it with the store disclosures. The threat model gains a network
  egress section and two residual risks, R13 (a telemetry key can be
  extracted and abused) and R14 (the bundled root set ages with the SDK).
- **`docs/bridges/README.md` gains C12**, the telemetry contract a binding
  owes (fill the platform fields itself, own the lifecycle, keep the blocking
  calls off a UI thread), and names the held inbound buffer as a third
  event-holding shape that must not be folded into the one-shot map.
- **`docs/licensing-faq.md` answers whether telemetry is part of the
  license**, and `docs/configuration.md` and `docs/mls-integration.md` state
  the new queue and dedup defaults and the two new sealed record kinds.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.25.x](docs/changelog/0.25.md) | 0.25.0 |
| [0.24.x](docs/changelog/0.24.md) | 0.24.1, 0.24.0 |
| [0.23.x](docs/changelog/0.23.md) | 0.23.0 |
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
