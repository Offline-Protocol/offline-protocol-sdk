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

- **Mesh forwarding is configurable and observable from every binding.**
  Multi-hop forwarding shipped with its tunables and counters as Rust-core
  surfaces, so an RN app could neither tune it nor see it working: with no
  counters to read, even a correctly staged three-device test could only infer
  forwarding from delivery.

  `ProtocolConfig.meshRelay` now carries the sixteen governor tunables (hop
  budgets, fan-out, jitter, rate and burst budgets, queue capacity, the
  capability-bias dials, and the activity window that decides relay standing).
  `getMeshRelayStats()` reports what this device has carried; note that a
  device with a working relay connection forwards nothing, because the mesh is
  only offered frames no other carrier can deliver, so zero counters on an
  online device are the honest answer rather than a fault.

  Every config field is optional and an omitted one keeps the core's default,
  so a section naming one dial moves only that dial and no binding ever
  restates a number it does not own. The read side is deliberately the
  opposite: `getMeshRelayTunables()` returns every field populated, read from
  the governor rather than echoed back from what was passed in, so no caller
  writes a fallback literal that could drift from the core. The section is
  applied at construction; there is no runtime update, because the governor
  snapshots it at build time and re-pointing it mid-flight would have to
  rebuild the token buckets and suppression cache underneath in-flight
  forwards. Suppression-cache sizing stays core-only, being internal memory
  sizing rather than a policy dial.

- **Username discovery and a self-certifying invite payload.** Two ways to
  reach a peer you have never spoken to, restoring reach-by-username, which the
  addressing migration removed.

  `createInvite()` and `parseInvite()` produce and verify a compact base64url
  blob carrying `{address, pubkey, petname?, sig?}`. It is verifiable offline
  by anyone and, like `deriveAddress`, needs no protocol instance, so a scanner
  can check a QR code before `create()`. The optional signature binds the
  petname to the key; it does not defend against substitution (an attacker's
  own correctly-signed invite is indistinguishable from a stranger's) but it
  does stop a forwarded invite saving Alice's key under the name "Bob". Sign
  when the invite may travel without its issuer. Invites deliberately carry no
  key package (an MLS init key is single-use, a QR code is static, so pairing
  them guarantees a collision the moment two people scan the same code) and no
  expiry.

  `resolveUsername()` looks a name up in a directory published over Nostr
  (addressable kind 30777, sealed, one record per device). Off by default via
  `transports.nostr.usernameDiscoveryEnabled`, and it additionally requires
  cold contact, since a claim points at an address whose key packages are what
  a resolver fetches next. Default-off is deliberate: publishing binds a
  human-readable name to an address in a public place, where the mapping *is*
  the payload, which is materially more disclosure than a key-package record's
  "an install with this tag exists".

  **The directory is not authoritative, and the API is shaped so you cannot
  forget it.** Anyone may claim any name, so a resolution returns the whole set
  of claimants as a single `username_resolved` event: no ranking, no "best"
  claim, and no per-claim event to race. An app that auto-selects has converted
  a non-authoritative directory into an authoritative-looking one, and its user
  then believes the *name* was verified when only a *key* ever was. Present the
  claims, let the user confirm out of band, and store the address rather than
  the name. Note that even a single user resolves to a set: a phone and a
  laptop are two genuine claims, and collapsing them hides the second device.

  `resolveUsername()` resolves `true` if it started the lookup and `false` if
  it joined one already in flight; **both mean the event is coming.** Every
  case where no event will ever arrive rejects instead, so awaiting the
  resolution can never hang on a lookup that was never started. The rejection
  code says which case, because they need different handling:
  `InvalidConfiguration` (discovery is off, so retrying cannot help),
  `InvalidState` (too many lookups in flight, retry shortly), and
  `InvalidArgument` (not a claimable name).

  The event also reports `truncated`, the number of *verified* claims dropped
  at the accumulator's ceiling. It is the opposite statement to `rejected`:
  those records passed every check and are missing anyway. Non-zero means
  `claims` is a sample, so an absence from it proves nothing, and it is the
  only signal that a name is being squatted at volume, which would otherwise
  render as a clean set.

  A resolution completes when **every** relay it asked has answered rather than
  when the first one has. That is a correctness property, not a latency choice:
  a claim needs only one honest relay to survive, so finishing early would let
  whichever relay answered fastest decide the whole answer, including a relay
  that holds nothing. A relay that never answers is bounded by a timeout, and
  one that disconnects stops being waited on.

  Each record binds the Nostr key it is published under, which the key-package
  record does not. Without that binding a third party could unseal a claim,
  re-seal the genuinely signed payload under their own key, and republish it,
  and because addressable replacement is per-author the owner's retraction
  would never displace the copy. Renaming or switching the feature off retracts
  the standing claim.

  Discovery events are additionally checked against their own BIP-340
  signature, with the event id recomputed rather than trusted. This is the one
  record kind that needs it: a retraction's body is a constant, so nothing
  inside it is signed and its whole meaning is *who published it*, while the
  seal key is public by construction. Without the check a single hostile relay
  could forge a retraction for an honest claimant and erase them from the
  resolved set even while every other relay served their genuine record —
  inverting what querying many relays is for, since a claim needs only one
  honest relay to survive.

  Invite petnames are screened for the control and format characters a
  username already refuses. A petname is what an app renders in the
  confirmation dialog after a scan, and on a signed invite a bidi override
  would otherwise arrive bound to a valid signature.

  Wire format, verification order and threat model:
  [docs/spec/username-discovery.md](docs/spec/username-discovery.md).

- **Capability bias in mesh forwarding.** Battery level and charging state now
  continuously scale how much of the mesh's traffic a device carries: the delay
  before it transmits a forward, the number of neighbors it fans out to, and
  the refill rate of its forwarding budget. A device on mains power usually
  transmits first, and its neighbors holding the same frame stand down having
  spent no airtime — so the saving is the forward that never happens. Nothing
  switches off at a threshold: a misjudged scale costs redundancy, where a
  misjudged threshold would remove a link the network needed. Devices that are
  charging or configured `relayPriority: 'always'` are exempt. Tunable via
  `mesh_relay.bias_min_scale` and `mesh_relay.bias_max_handicap` (Rust config;
  `1.0` disables bias entirely). Unfed devices are unaffected — an unknown
  battery level means full effort, as it already did for the forwarding gate.

  The scaling reaches **only traffic carried for other devices**. A device's
  own messages and acknowledgements keep the full fan-out and the full send
  rate at any battery level: for a forward a narrower fan-out costs redundancy
  because neighbors hold copies and the sender is still retrying, but for a
  frame this device originated the fan-out *is* the delivery attempt. The
  forwarding share is metered separately from the device's own airtime ceiling
  so the two cannot be confused.

  `ProtocolConfig::validate` now also refuses a `mesh_relay.jitter_max +
  mesh_relay.bias_max_handicap` that reaches the 5s overdue cut-off, past which
  a biased forward would be abandoned on release rather than merely delayed.

- **`setBatteryState(level, isCharging)` — the battery feed the relay and DORS
  policies were always waiting for.** See Fixed below for why this is the
  headline change rather than a convenience. `getIsCharging()` reads it back.
- **`updateRelayConfig(config)` / `getRelayConfig()`.** Whether this device
  carries other people's traffic — the battery floor, the outright opt-out, the
  priority mode — is now settable at runtime rather than only at construction,
  and omitted fields keep their current values. Applies to the next forwarding
  decision and the next `process()` tick.
- **Three DORS fields reached the FFI**: `lowBatteryThreshold`,
  `relayMinBatteryLevel`, `relayOptimalConnectionCount` — including the
  React Native `updateDorsConfig` / `getDorsConfig` signatures, without which
  the round trip that keeps a field alive across an update is unexpressible.
- **`NetworkNode` carries `battery_level` and `connection_count`.**

### Changed

- **Design documentation restructured; `CLAUDE.md` reduced from 61 KB to 7.6 KB.**
  The durable knowledge that had accumulated in one working document now lives in
  reviewable, versioned documents under `docs/`, and nothing was dropped except
  the per-feature test-name lists (the docs name the behaviour that is pinned;
  grep finds the tests). New material:
  [`docs/spec/`](docs/spec/README.md) is an implementation-independent protocol
  specification (identity and addressing, the message model with both encodings
  and the frozen binary layout, the reserved prefix registry with the signing
  gate and its two exemption classes, the three encryption envelopes, the group
  protocol, capability negotiation);
  [`docs/security/threat-model.md`](docs/security/threat-model.md) states the
  adversary classes, trust boundaries and residual risks plainly;
  [`docs/state-machines/`](docs/state-machines/README.md) documents delivery and
  acknowledgement, outbox and retries, session lifecycle, group message
  lifecycle and transport lifecycle with diagrams;
  [`docs/adr/`](docs/adr/README.md) records fifteen decisions that are expensive
  to reverse or easy to undo by accident; and
  [`docs/bridges/`](docs/bridges/README.md) writes down the Rust-to-Swift,
  Kotlin, Python and TypeScript contract, every rule of which fails *silently*
  when violated. `CLAUDE.md` is now repository instructions plus a pointer
  table. Five Rust doc comments that said "see CLAUDE.md" now name the specific
  document, and two more are corrected rather than redirected: `Address`'s
  ordering note told readers to compare `Address` values at all four protocol
  tiebreakers, which is true of one of them and is the exact change
  [ADR 0003](docs/adr/0003-self-certifying-addresses.md) warns breaks
  convergence; a DORS comment named a default switch hysteresis of 10 when it is
  15.0. No behavioural change.

- **`CHANGELOG.md` is archived by release series.** The working file was 2,425
  lines and growing without bound; it now holds unreleased changes plus the
  current release, and older releases live in
  [`docs/changelog/`](docs/changelog/README.md), one file per minor series with
  its own release table. Content is unchanged. **Release-cut procedure gains a
  step:** after cutting, move the now-previous release's section into
  `docs/changelog/<major>.<minor>.md` and update both archive tables.

- **BREAKING: `relay_promoted` / `relay_demoted` / `isRelay()` report observed
  forwarding, not predicted capability.** They previously fired from a
  threshold on connection count and battery, which meant a device surrounded by
  peers that never needed it announced itself a relay having forwarded nothing,
  while a device carrying the whole room's traffic over two links announced
  nothing at all. They now report frames this device has actually carried
  within a rolling window (default: 3 frames per 60s to begin; 2 consecutive
  quiet windows to end). Consequences for apps:
  - They **no longer require a battery feed**. Forwarding is observable without
    one, so `relay_promoted` can now fire on a device that has never reported a
    level — inverting the previous "no feed, no events" behaviour.
  - `relay_promoted.battery_level` is now **nullable** (`number | null` in
    React Native), being `null` on such a device.
  - `isRelay()` answers `false` on a capable device that has had nothing to
    carry — including any device with a working internet relay, since the mesh
    is only offered frames nothing else can deliver.
  - `relay_demoted` no longer reports `"connections below relay threshold"`;
    sustained quiet reports `"no traffic carried for other devices recently"`.
    Demotion for `allowRelay: false` and for the battery floor is unchanged and
    still immediate.
- **BREAKING: `relayThreshold` removed** from `RelayConfig` across the Rust
  config, UniFFI/UDL, Swift, Kotlin, Python and React Native. It configured the
  connection threshold of the promotion state machine above, which no longer
  exists; nothing else read it. (This also removes a `relay_threshold as usize`
  cast that wrapped on 32-bit targets, where an absurd configured value became
  an aggressive promotion threshold rather than an unreachable one.)
- **BREAKING: `ProtocolConfigExtended` removed** from the UDL and FFI. It
  carried `relay` and `dors` sections but no constructor ever accepted it, so
  no caller on any binding could supply one — a create-time configuration
  surface that looked real and did nothing.
- **`relayPriority: 'always'` now also exempts a device from the *forwarding*
  battery floor**, relaxing it to the hard 15% floor exactly as charging does.
  It previously affected only the relay-role label while the 30% soft floor
  still stopped the device forwarding, which is weaker than the setting's
  documentation implied.
- **React Native `updateRelayConfig` now throws on an unrecognised
  `relayPriority`** instead of silently dropping it, matching
  `setRelayPriority`. Dropping it applied the rest of the update and left the
  priority at its old value, which from the call site is indistinguishable from
  having set it. Unreachable from typed TypeScript; reachable from JavaScript.

### Fixed

- **Three mesh-relay behaviours that shipped guarded by nothing.** A
  `mesh_relay.fanout` of zero is now refused at construction: it read like an
  off switch but had quietly become a fan-out of one, and before that it was a
  silent drop of an already-admitted frame. The capability-bias window
  separation is now asserted against the handicap a device can actually pay
  ((1 - `bias_min_scale`) x `bias_max_handicap`) rather than the configured
  ceiling, which no device ever reaches. The old assertion stayed green while
  a raised min scale would let a weak device's jitter window overlap a capable
  one's. And `RelayPriority::Always` excusing the *forwarding* battery floor,
  not just the cosmetic relay label, now has gate-level coverage in both
  directions: eager below the soft floor carries, eager below the hard critical
  floor still does not.

- **Battery-aware relaying actually runs now.** Every battery-dependent policy
  in the SDK reads the device's charge from the per-transport metrics map, and
  nothing in production ever wrote to it: `setBatteryLevel` stored the number
  in a field only a small FFI-local helper read, and `updateTransportMetrics`
  has been a documented no-op for several releases. So on every real device
  DORS energy scoring skipped its battery term, the relay role was never
  evaluated at all — `relay_promoted` and `relay_demoted` could not fire, and
  `QUICKSTART.md` documented events that could never arrive — and message
  forwarding always took its "unknown battery means willing" branch, meaning
  the floor that stops a device spending its last few percent carrying other
  people's traffic was never applied. The host feed now reaches the engine, so
  a transport that reports no battery of its own inherits the device's; one
  that reports its own keeps it.

  Apps must call `setBatteryState` (or `setBatteryLevel`) on start and on each
  platform battery notification — without it these policies stay in their
  unknown-level branch, exactly as before. Report charging state where the
  platform provides it: a charging device is deliberately excused the soft
  `minBatteryForRelay` floor, and reporting only the level strips relay duty
  from plugged-in devices that should keep it.

- **React Native relay configuration is no longer dropped between JS and
  native.** `allowRelay`, `minBatteryForRelay` and `relayThreshold` were
  documented, accepted, carried across the bridge — and then parsed by nothing,
  because no API to apply them existed; only `relayPriority` was read. Every
  mobile app therefore ran on the default relay configuration whatever it
  configured. All four fields now apply, at create time and at runtime.

- **`updateDorsConfig` no longer silently resets three fields.**
  `lowBatteryThreshold`, `relayMinBatteryLevel` and `relayOptimalConnectionCount`
  were absent from the FFI shape, so every runtime DORS update quietly reset
  them to 20/30/4 — including an update that changed something else entirely.

- **Topology nodes no longer report battery level as signal strength.** The
  FFI `NetworkNode` had no battery field, so `get_topology()` mapped
  `battery_level` into `rssi` — a battery of 80 surfaced as −80 dBm. Both are
  now their own field (`rssi` is `null`, since link quality is tracked per
  link, not per node).

- **Only the recipient can mark a message delivered.** A delivery
  acknowledgement names a message id and nothing else tied it to a delivery, so
  an acknowledgement from any party that had merely seen the frame settled it:
  the outbox entry was dropped, the retries stopped, and the app was told
  `message_delivered` for a message that arrived nowhere. Every device that
  carries a frame across the mesh knows its id, and the mesh fall-back below
  hands frames to exactly those devices — so the two belong together.
  Acknowledgements naming a message sent to somebody else are now ignored on
  both settlement paths, and a rejected one leaves the message's retry record
  untouched so it keeps being retried. This is an attribution check rather than
  authentication: acknowledgements carry no signature, so it removes the
  unattributed answer, not a determined forgery by someone who saw the frame.

- **A device with internet no longer keeps messages from the mesh standing
  next to it.** Reachability was decided from local carrier status alone: any
  carrier that does its own routing being up — Internet, Nostr, Reticulum —
  counted as reachable for *every* recipient, so nothing was ever handed to
  the neighbors. That is right in the case forwarding was built for, where
  nobody has infrastructure, and wrong in a mixed neighborhood. Two halves,
  and the second was the one that bit: a message to a peer reachable only
  across the mesh went to the relay, earned the "recipient unreachable"
  verdict, and parked waiting for someone who was never going to come online;
  and an online *recipient* answered a mesh-delivered message over the relay,
  where an offline sender could not see it — so the sender retransmitted a
  message that had been delivered and read, and eventually reported it failed.

  The initial send is unchanged, so an online device still does not spend its
  neighbors' battery on traffic the relay serves. What changed is what happens
  after the relay contradicts it:

  - The relay's `recipient_unreachable` verdict — the one per-peer
    reachability fact a device receives — now hands the message to the mesh as
    well as parking it, on every park rather than only the first, so a
    recipient who was out of range at the first park is still reached later.
    Media chunks are offered the same way.
  - An acknowledgement for a message that arrived across the mesh travels back
    the way it came, whatever the answering device's own carriers say. When
    that device is also online the answer goes both ways; the duplicate costs
    one frame, which the sender's ack handling already absorbs.
  - Because parking removes the pending ACK, a parked message delivered across
    the mesh is now settled from the acknowledgement alone, firing the ordinary
    `message_delivered`. Without this the mesh hand-off would have delivered
    messages the sender never learned about.

  Still not covered: a device whose only infrastructure is Nostr or Reticulum
  gets no unreachable verdict from either, so nothing contradicts the initial
  answer and no fallback fires. Carrier status is also platform-reported and
  means "this carrier is up", not "the relay connection is authenticated".
  (#326)

## [0.21.0] — 2026-08-13

> **Your identity is no longer something your app picks.** `ProtocolConfig.userId`
> is replaced by `profile`, and a device's identity on the wire becomes the
> self-certifying `off1…` address derived from an Ed25519 identity key it mints
> for itself. Peers verify an address by re-deriving it from the key its owner
> presents, so an address cannot be claimed by anyone who does not hold its key
> — where before, impersonation cost typing a name.
>
> `profile` keeps the old field's other job: choosing which stored identity this
> instance runs as. It never leaves the device, and passing the string you used
> as `userId` keeps you in the same storage namespace (the namespace hash is
> unchanged). Read your address with `localAddress()` or from the new
> `identity_ready` event.
>
> **There is no in-place migration and that is deliberate**: the identity is the
> MLS credential, so changing it invalidates every existing session no matter
> what, and 1:1 session slots are named after the two ids. Old containers are
> left intact rather than deleted; clean them up by passing the *old* user id to
> `wipePersistedState`. Full migration guide in
> [UPGRADING §14](./docs/UPGRADING.md#14-your-identity-is-derived-not-chosen-v0210).

### Changed

- **BREAKING**: `RelayPriority` is spelled `never` / `auto` / `always`
  everywhere, matching the engine's own vocabulary and the `relayPriority`
  config field apps already write. The FFI enum previously used an unrelated
  `low` / `medium` / `high` triple that fed a heuristic disconnected from the
  real relay policy, so a device could report one priority and behave by
  another. The old spelling is still **accepted on input** by both bridges and
  by JS `setRelayPriority`; what changes is what `getRelayPriority` returns —
  update any code that compares its result against `'medium'`. Direct
  Swift/Kotlin/Python consumers of the `RelayPriority` enum must use the new
  case names.

- **BREAKING**: `set_battery_level` is now fallible (`[Throws=ProtocolError]`),
  because it reaches the engine rather than an FFI-local field. Direct Swift
  callers need `try`, and Kotlin/Python callers get the same checked error every
  other engine-touching method raises. React Native callers are unaffected —
  `setBatteryLevel` was already a promise.

- **`isRelay()` reports the engine's actual relay role** — the one
  `relay_promoted` / `relay_demoted` announce — instead of a separate FFI-local
  guess based on BLE peer count. The two could disagree; now they cannot.
  It stays `false` while no battery level has been reported, because the role
  policy declines to transition on a level it does not have.

- **`updateDorsConfig` and `updateRelayConfig` are partial updates on both
  platform bridges**: a field the payload omits keeps its **current** value,
  read back from the live engine. Both bridges previously rebuilt the whole
  `DorsConfig` from hardcoded literals, so an update meaning to change one
  field silently reset every other one — the same defect as the three missing
  fields above, one layer up and affecting all eighteen. Apps that relied on
  `updateDorsConfig` to restore defaults must now pass the values explicitly.

- **BREAKING**: `NostrTransport::get_next_message` — the generic
  `Transport` whole-message poll — now returns
  `Error::ConfigurationError` instead of the next queued frame. It never
  sealed, signed, or wrapped: it returned the bare serialized `Message`, the
  whole protocol envelope with both endpoints on it, so anything that
  published the result put in front of every relay exactly the cleartext
  sealing exists to prevent — and did so regardless of
  `nostr_sealing_enabled`, which this path never consulted. It had carried a
  doc comment warning callers off it since gift wrapping landed, but the
  method sits on the `Transport` trait and the engine hands that out as
  `dyn Transport` from `TransportManager::get_transport`, so reaching the
  cleartext took no downcast and no unsafe. **No bundled bridge or UniFFI
  entry is affected** — `nostrGetNextMessage` routes to
  `get_next_signed_event`, which returns a signed, sealed `["EVENT", …]`
  relay message, and the iOS and Android bridges call it. Only a Rust
  embedder polling Nostr through `dyn Transport` changes behavior, and such
  an embedder was publishing cleartext. The refusal returns before the send
  queue or the pending-confirmation map are read, so it cannot strand the
  frame it declines to hand over: the message stays queued and the next
  `get_next_signed_event` serves it. The refusal is also logged at `error`
  level **once per transport**, because every in-tree caller of this trait
  method takes the shape `if let Ok(Some(..)) = t.get_next_message()`, which
  discards an `Err` down the same silent path as `Ok(None)` — the log is what
  makes the misintegration visible to a caller that swallows the error, and
  the one-shot is what keeps a polling caller from burying it.
- **BREAKING**: `GroupManager::remove_member` now takes `&[LeafNodeIndex]`
  instead of a single `LeafNodeIndex`, and `MlsManager::remove_group_member`
  removes *every* leaf whose credential names the member rather than the first
  one found. One identity holding two leaves is refused by the wire gates, but
  not by a tampered local store — and there first-match leaves the peer in the
  group holding live keys while the roster shows them gone.
- **BREAKING**: `SecurityWarningCode` gains `GroupLeafIdentityUnproven`
  (`GROUP_LEAF_IDENTITY_UNPROVEN` on the wire and in the RN union). The Rust
  enum is not `#[non_exhaustive]`, so a downstream exhaustive match must add an
  arm; JSON and RN consumers are unaffected beyond handling the new string.
- **BREAKING**: non-`Member` MLS senders (`NewMemberCommit`,
  `NewMemberProposal`, `External`) are now refused with
  `MlsError::UnsupportedSender` rather than skipped, as is a commit or Welcome
  carrying an `ExternalSenders` group-context extension. This SDK issues none of
  them and configures no external senders, so no honest peer is affected; a peer
  running something else that did will now be declined instead of silently
  ignored.
- `secure_session_failed` can now fire while the session with that peer stays
  **live**: a session Welcome refused for carrying an unprovable identity is
  refused non-destructively, so the pre-existing session survives. It always
  reported a failed *attempt* rather than a terminated session; this is the
  first case where the distinction is observable, and apps that tear down
  session state on the event alone must stop. A
  `GROUP_LEAF_IDENTITY_UNPROVEN` security warning accompanies it.
- `GroupInfo` gains `unproven_members: u32` — how many leaves the roster read
  skipped for not deriving their own credential. Additive and `#[serde(default)]`;
  not carried on the UniFFI `MlsGroupInfo` record, so no bindings change. A
  non-zero count is reported to the app as `GROUP_LEAF_IDENTITY_UNPROVEN`
  attributed to this device, not to a peer.
- **BREAKING**: `ProtocolConfig.user_id` → `ProtocolConfig.profile` across Rust,
  UniFFI (both config dictionaries), React Native, and Python. `profile` selects
  a storage namespace and is never sent; it is not an identity.
- **BREAKING**: the wire identity is now the address derived from this profile's
  identity key. `Message.sender`, `recipient`, and `NeighborDiscovered.peer_id`
  all carry `off1…` addresses. Anything an app keyed by a *peer's* username —
  conversation rows, contacts, group membership — must be re-keyed. Anything
  keyed by the app's *own* id — per-user database filenames, MMKV namespaces,
  cache directories — must **not** be: keep passing that string as `profile`.
  Re-keying self-scoped storage to the address silently opens an empty store,
  which reads as a first launch rather than an error. Both traps are spelled
  out in UPGRADING §14.
- **BREAKING**: the MLS credential is the address, so peers on an older build
  fail cleanly at credential verification instead of interoperating.
- **BREAKING**: BLE discovery announces the derived address. The advertised
  `DEVICE_ID` characteristic carries this device's `off1…` address rather than
  the profile, the central verifies it against the identity key it reads before
  announcing the peer, and a mismatch drops the link. A peer still advertising
  a profile is therefore *invisible* over BLE rather than degraded — its
  control frames were already being rejected. Publication is gated on an
  identity existing, and the `bleRecipientNotAmongPeers` diagnostic is exposed
  over UniFFI. Wi-Fi Direct stops announcing or ingesting transport peer ids
  entirely — nothing on that transport can prove one — and the `wifiDirect*`
  React Native entry points are deprecated.
- 1:1 session slots and the both-create tiebreaker order addresses by their hash
  bytes rather than their rendered string. The bech32 charset is not
  ASCII-monotonic, so string order contradicts every other address comparison.
- The OpenMLS storage adapter hashes its keys, removing the ~8× inflation of
  `hex(json(GroupId))` now that ids are 44 characters.
- `wipePersistedState(appId, profile)` — same shape; pass a legacy user id to
  reach a pre-migration container.
- **BREAKING**: the five relay JSON payload formatters — `check_presence`,
  `request_prekey_bundle`, `upload_keys`, `set_typing`, `clear_typing` — are
  removed from UniFFI, React Native, and Python. They built JSON strings for a
  relay protocol the SDK does not speak: nothing in this repo called them, both
  bridges implement presence natively, and the last two of them were the only
  public parameters still literally named `username`. Apps talking to a relay
  directly should build these frames themselves; the SDK-mediated equivalent
  for typing is `sendTypingIndicator`.
- `conversation_id` documentation no longer tells apps to use "the recipient's
  username". The field is opaque to the SDK — carried to the peer and echoed
  back, never parsed — and the old guidance pointed at a value that has both
  changed and become unstable. Behaviour is unchanged; existing keys keep
  working.
- **BREAKING**: trust-on-first-use is deleted. Deriving an identity from its key
  makes a pin store redundant — a control message is now accepted only if its
  Ed25519 signature verifies *and* the signing key re-derives to the address in
  `sender`. Unlike a pin, that check has no first-contact window, which was the
  window impersonation lived in: claiming a name went from winning a race to
  finding a 160-bit second preimage (~2^160). That is the targeted figure; the
  birthday bound on the same 160-bit truncation is ~2^80, which buys two keys
  sharing one address rather than a specific peer's address — stated so it is
  not left to be derived. See `Address::HASH_LEN` for the trade.
  Removed with it: `resetTofuForPeer` (Rust,
  UniFFI, React Native, Python), the `tofu_reset` event, and the
  `TOFU_KEY_MISMATCH`, `TOFU_STORE_FULL`, and `SIGNATURE_DOWNGRADE` warning
  codes.
- **BREAKING**: `TOFU_KEY_MISMATCH` becomes `SENDER_ADDRESS_MISMATCH`, and it no
  longer has a benign reading. The old code could not distinguish a reinstall
  from an impersonator, which is why a reset action had to exist; an address is
  the hash of its key, so a peer that re-keys arrives as a *different address*.
  Treat this code as an impersonation attempt — do not offer "trust anyway".
- **BREAKING**: unsigned control frames are refused unconditionally. They were
  previously accepted from any peer without a pin. Consequence: `initializeMls`
  is effectively mandatory, since an instance with no identity key cannot sign
  and every peer will drop its control traffic. The relay server's own answers
  (`__GROUP_CREATED__`, `__GROUP_MEMBER_ADDED__`, `__GROUP_MEMBER_REMOVED__`,
  `__GROUP_INFO__`, `__USER_GROUPS__`, `__GROUP_ERROR__`) are exempt because no
  peer signs them — narrowly, on relay ingest only and only when the relay did
  not attribute the frame to a peer. Closing that residual means moving relay
  answers onto dedicated FFI entry points, as
  `internet_group_report_received` already does; that is follow-up work.
  Because the exemption also requires the frame to be unattributed, the RN
  bridges now inject **every** relay answer with a null actor — previously
  `__GROUP_MEMBER_ADDED__` and `__GROUP_MEMBER_REMOVED__` carried the
  relay-reported `added_by` / `removed_by` as a reachability assertion, which
  set a transport peer identity and would have had those frames dropped as
  unsigned. The actor still rides the payload, which is what the handlers read;
  `__GROUP_MSG__` keeps its attribution (a data-plane prefix is never gated) and
  remains the reachability signal for a relayed sender.
- **BREAKING (behaviour): relay-native member-removal reconciliation is inert.**
  `__GROUP_MEMBER_REMOVED__` is authorized off the *wire* `sender`, which must
  now be an admin; the relay's own answer is injected unattributed, so its
  placeholder sender never is. The frame is dropped and **no
  `group_member_removed` event fires** for it. Previously the bridges passed the
  relay-reported `removed_by` as the sender, so the reconciliation could take
  effect for an admin nobody had pinned yet. The working path is unchanged and
  is the one the SDK itself uses: the removing admin's own signed direct
  notification (`removeMember` → a signed `__GROUP_MEMBER_REMOVED__` to the
  removed member). Apps that relied on relay-orchestrated removal to update a
  roster must either drive removals through the SDK or await the follow-up that
  moves relay answers onto dedicated FFI entry points. `__GROUP_MEMBER_ADDED__`
  is unaffected — its handler reads the payload and runs no sender check.
- `requireTransportIdentity` keeps its `false` default and no longer gates the
  signature requirement, which is now unconditional. Its one remaining effect is
  to reject frames arriving with no transport peer identity — which on a
  deployment running Nostr or sender-less relay delivery rejects that entire
  control plane, so it stays off by default.
- The durable record behind the inbound-plaintext gate is now a purpose-built
  category (`encryption_capable_peers`) rather than a side effect of the pin
  store. Same property, stated directly: a peer that has proved it runs MLS is
  still known to after a restart, so a remotely-triggered session teardown
  cannot re-open the cleartext path for them.
- `MlsError::KeyPackagePinMismatch` → `MlsError::KeyPackageAddressMismatch`. Key
  packages are checked by re-deriving the address from the leaf signature key,
  which — unlike the pin it replaces — also runs on first contact and on every
  read of a cached package.
- **BREAKING**: the Nostr routing tag is derived from the address rather than
  the app-chosen id, and the Nostr transport now **requires** the protocol
  identity. It is installed by the identity rebuild rather than by the
  constructor, so `enableTransport('nostr')` before `initializeMls` — including
  any config with `encryption.enabled: false` — is refused instead of falling
  back. See Security below for why the fallback had to go rather than be fixed.
- **BREAKING**: a Nostr send whose *recipient* is not a derived address is
  refused rather than published. The recipient is the sole preimage of the `#p`
  tag the frame is addressed under, so a username-shaped one carries the same
  disclosure the local id was refused for, and any non-address one addresses the
  frame where no peer subscribes. Refused at `send`, so the caller can route
  over another transport; the same rule gates the key-package resolution queue.
- **BREAKING**: the publicly-computable key that seals published key-package
  records and bootstrap frames is no longer the routing tag. Both are still
  derived from the address, but by separate derivations — the tag is still
  rooted in a bare `SHA-256(address)` with no domain separation, the seal key
  is a domain-separated HKDF — so a routing tag no longer doubles as a live
  encryption key. **The tag's own private half remains computable** and is not
  claimed otherwise: its scalar *is* `SHA-256(address)` and the tag is that
  scalar's x-only public key, so anyone holding an address can reconstruct the
  keypair behind that address's tag. What changed is that nothing is sealed to
  it any more, so reconstructing it decrypts nothing. The old shape was not
  exploitable — nothing signs with a tag, and inbound is never authenticated
  by pubkey — but a routing label that doubles as a live encryption key is a
  trap for anything that later adds NIP-42 AUTH or pubkey-based filtering.
  `derivable_for_device_id` is replaced by
  `record_seal_keypair_for_address`, and `routing_tag_for_device_id` by
  `routing_tag_for_address`.
- **BREAKING (behaviour): cold contact by username is gone**, because a
  username no longer resolves to anything. Cold contact by *address* works
  exactly as before — a peer reachable from an invite or QR code still resolves
  over published key packages with no prior exchange. Published records stay
  sealed, though for a narrower reason than the one documented: the
  username-directory leak they were built to prevent expired when credentials
  became addresses, and what remains is that a cleartext record would publish
  its own address at a tag that is otherwise one-way.

### Security

- **Telemetry sinks received display names in the clear.** `sender_name` and
  `accepted_by_name` were passed through raw by the `scrub_ids` scrubber, on
  the reasoning that a display name was decoration beside a `sender` that was
  itself the username. Deriving identity from a key inverted that: `sender` is
  now a pseudonymous `off1…` address, which leaves the petname as the only
  field in the event that names a *person* — the most identifying value the
  sink receives rather than the least. Both are now hashed like every other
  actor field, so a sink still correlates records without learning who they
  are. Third-party sinks parsing these fields as human-readable will see opaque
  hex; set `scrub_ids: false` to opt out, as for every other identifier. Group
  and file names are unchanged, and stay raw: they label a shared thing, not an
  individual.
- **Telemetry event `reason` fields no longer carry identifiers or text a
  remote party wrote.** These fields ship to sinks verbatim by design — the
  scrubber hashes identifier fields, not prose — and several producers rendered
  errors into them that interpolated addresses, session slots, or wire text:
  session-Welcome failures (`secure_session_failed`), the relay's
  `__GROUP_ERROR__` wording (`GroupError.reason`), the control-gate and
  relay-binding `security_warning` arms, and transport/relay send-failure text
  on `messageDeferred`, `messageUndeliverable`,
  `connectionRequestUndeliverable` and `welcomeSendFailed`. Each now carries a
  fixed, locally chosen classification; the full wording stays in the device
  log, and relay errors remain available verbatim through the raw
  server-message frame both bridges already emit. `GroupError` gains an
  additive `group_id` field, hashed by the scrubber like any other identifier.
  Sinks parsing these strings will see the new vocabulary; the docs have
  always said `reason` must not be parsed.
- **Group members were trusted on an identity claim nobody checked.** The
  addressing work made a claim provable — an address is the hash of its
  identity key — and wired that check to the two places a claim was known to
  arrive: control frames, and key packages this device imports. A third was
  missed. A leaf reaching the MLS ratchet tree any other way — in a Welcome's
  tree, or in an Add another member commits — was never re-derived, so the
  credential stayed what RFC 9420 calls it: "a bare assertion of an identity".
  SEC-M1 compares the wire sender against exactly that credential, which meant
  it was checking a name the forger had chosen against a name the forger had
  chosen. Reachable without a signature from anyone, since `__GROUP_MSG__` is
  data-plane and its exemption rests on SEC-M1; and cheap to reach, needing
  only a committed Add (membership commits are unauthorized by default) or an
  accepted group invite. Every leaf entering local group state is now bound to
  its own signature key at three points — joining a Welcome, merging a commit,
  and attributing a decrypted message — which is the Authentication Service
  RFC 9420 §5.3.1 leaves to the application and OpenMLS declines to perform.
  The check is unconditional, unlike the admin-commit enforcement beside it,
  because its verdict comes from the commit's own bytes rather than from
  replicated state: every honest member reaches the same answer, so a refusal
  forks the attacker off a group that stays consistent. Refusals surface as
  `GROUP_LEAF_IDENTITY_UNPROVEN` from all three refusal sites — a declined group
  invite, a refused membership commit, and a declined session Welcome — with the
  group sites rate-limited per `(group, sender, site)` on the same 300s window as
  the unauthorized-membership report, since a refusal is permanent and costs an
  insider nothing to repeat. A forged commit's *first* delivery is **not** buffered
  for retry: it can never succeed, and a buffered commit that expires after
  retries is read as an epoch fork, which would have let one forged commit
  trigger a group-wide re-key round and a false alarm. (A re-sent copy of the
  same frame still fails earlier as a spent ratchet generation and is buffered
  like any replayed commit — pre-existing behaviour common to all commits, not
  specific to the binding.) Coverage includes the update path, where a
  member renames *their own* leaf to a peer's address — no new leaf, no key
  package, no invite, and the cheapest form of the attack.
- **`SecurityWarning.peer_id` printed in the clear via `Debug`.** Every other
  peer-bearing event redacts it, and the telemetry scrubber hashed this one
  correctly — only `{:?}` disagreed, which is the formatting an operator
  reaches for while investigating precisely this event. The peer named is also
  frequently attacker-controlled, since an injected frame carries whatever
  sender it likes. Now redacted, with a test covering all thirteen
  peer-bearing variants rather than the one that broke.
- **A username-derived routing tag could reach public relays, and did.** The
  tag is `SHA-256(id)` and both bridges send the subscription filter carrying
  it to every relay the moment a socket opens, unconditionally. Since the
  identity rebuild the id is the derived address — but the rebuild is
  skippable: `encryption.enabled: false` skips `initializeMls` outright, and a
  thrown one is caught and warned past by the React Native layer, after which a
  retry is refused for the life of the instance. Either way the transport built
  from the app's **profile** was still installed when the relays connected, so
  a label anyone could recompute from a username was published to third-party
  archival relays with nothing to retract it and no error anywhere. The bundled
  `nostr-example` app shipped exactly that configuration against three public
  relays. Nostr now refuses to run without an identity, and the transport
  refuses an id that is not a derived address rather than hashing it into a
  tag.
- **The relay broadcast path now requires the `group_delivery_v3` capability**
  (previously `group_delivery_v2`). v3 is the same settled delivery-report
  contract with an address-aware relay group path: members are named by the
  identifiers the roster was registered under — for this SDK the MLS roster's
  `off1…` addresses — fan-out and push resolve those names relay-side, and
  group sender attribution carries the sender's declared address. Against a
  v2 relay the group path and the address identity cannot compose: the relay
  cannot route to address-registered members, its report names members in a
  namespace that never intersects the MLS roster (so the backstop re-sent to
  the entire roster after every broadcast), and the copies it did deliver
  arrived mis-attributed and were rejected after spending their one
  decryption (see Fixed). The gate now fails closed against such a relay and
  every group send takes the always-correct per-member fan-out until the
  relay upgrades. No configuration change; `group.relayBroadcastEnabled`
  keeps its meaning.

### Added

- **Messages now travel between devices that cannot hear each other.** A frame
  addressed to a peer out of range is handed to nearby devices, which carry it
  onward — and the delivery acknowledgement takes the same path back, so a
  message that arrived across several devices is not retransmitted as though
  it had been lost. Forwarding is governed: an id travels once, a frame waits
  briefly so a nearer neighbor can cover it first, the hop budget a frame
  claims is clamped to local policy, and the rate frames leave at is capped in
  total and per neighbor. Carried traffic never settles this device's own
  outbox or clock. Tunable via `ProtocolConfig.mesh_relay`;
  `mesh_relay_stats()` reports what a device has been carrying. Apps that
  implemented their own forwarding should remove it — a second forwarder
  transmits copies the budgets do not account for. See `docs/mesh.md`.
- **The Rust workspace publishes to crates.io, and its version is now the
  release version.** A `publish-crates` job runs beside the npm publish, behind
  the same build-and-test gates, and ships all eight crates in dependency order.
  Two things had to change for that to be safe. The workspace version moves from
  the decoupled internal `0.2.0` to `0.20.1`, in lockstep with the git tag and
  the npm package — two numbers for one release is a fine trade while nothing is
  published, and a bad one once `cargo add offline-protocol` is how people
  consume the SDK, because crates.io would then carry a version nothing else in
  the project answers to. (`bindings/python/pyproject.toml` moves with it; it
  tracked the Cargo version and would otherwise have become a third scheme.) And
  because crates.io versions are immutable — yankable, never replaceable — the
  release workflow now *verifies* the version rather than writing it: publishing
  is refused unless `[workspace.package].version` and `pyproject.toml` both
  equal the tag, unless the ref is a tag at all, unless the tag is free of any
  prerelease suffix or build metadata, and unless packaging has already verified
  cleanly with `--locked` and no credential in the environment. The version
  check is its own job, ahead of *both* publishes, so a forgotten bump stops npm
  too — checked inside the crates job alone it would fail there while npm
  shipped the number regardless, leaving a version that can never be completed
  on crates.io and can only be abandoned. Bumping the version is part
  of the release cut, now written down in
  [CONTRIBUTING](./CONTRIBUTING.md#cutting-a-release) — which also records why
  force-moving a released tag, previously the cheap fix for a post-release
  failure, is no longer one. Consumers of the npm, Python, or binary artifacts
  see no change; the crates are a new distribution channel, published under the
  same AGPL-3.0-only terms.
- `getBleDiagnostics()` (React Native) returns the three BLE degraded-path
  counters — `fragmentFallbacks`, `recipientNotAmongPeers`,
  `undersizedMtuReports` — which previously stopped at the UniFFI layer and
  were unreadable from an app. They are the rollout alarm for this release:
  each counts a frame that was still *sent*, so a fleet whose peers disagree
  about identity falls back on every send while its delivery metrics stay
  clean. Watch the trend across a release, not the absolute value.
- `localAddress()` (Rust, UniFFI, React Native, Python) returns this device's
  address, or null before startup opens the storage that holds the key.
- `identity_ready` event, carrying the address at the moment it is known.
- `MlsError::IdentityAddressMismatch`, raised when a stored identity key does
  not derive to the address it is being used under — the wrong namespace for a
  profile, or a replaced key — instead of building a credential the device
  cannot prove it owns.
- **The relay's answer to the address declaration is now checked, not just
  logged.** Declaring proves what this device *claims*; the relay's
  `AddressDeclared` echo is the only evidence of what it actually **bound**, and
  that echo used to terminate in a bridge diagnostic — inside `InternetManager`,
  the one class of code neither platform can execute in CI. So a relay that
  bound something else produced exactly the failure this whole chain exists to
  remove (frames attributed to an identity the receiver will not match, and no
  MLS session establishable over the relay) while being visible nowhere but a
  device console. Both answers now reach the SDK through dedicated FFI entry
  points — `internet_address_declared`, `internet_address_declaration_refused`,
  on the `internet_group_report_received` precedent, so an acknowledgement
  cannot be synthesized through the notification ciphertext injector — where the
  echo is compared against `local_address()` and two new `security_warning`
  codes report the outcome. `RELAY_ADDRESS_BINDING_MISMATCH` has no benign
  reading: the relay verifies that the declared address derives from the key
  that signed the proof, so an echo naming anything else means it bound what it
  did not verify. `RELAY_ADDRESS_DECLARATION_REFUSED` is operational rather than
  adversarial; the relay's own wording stays in the device log, and the event
  carries a fixed classification (see the telemetry `reason` entry under
  Security). Neither is acted
  on — the refusal path is deliberately non-fatal on both sides, and against a
  hostile relay a local teardown protects nothing it does not already control —
  so the whole value is that the signal is loud and typed. Worth surfacing in
  app-side relay health, because "the connection works" and "the connection can
  start new conversations" are different claims: an undeclared connection keeps
  delivering on **established** sessions and cannot establish new ones.

- `initialize_mls` must now be called **before** `start()` and returns
  `InvalidState` otherwise. Deriving the address is what lets the transports be
  rebuilt to carry it, and rebuilding replaces transport objects the platform
  has already reported connection status onto — against a running protocol that
  would strip every transport back to disconnected-with-an-empty-queue and
  report nothing. React Native already calls them in this order inside
  `start()`; direct UniFFI and Python embedders that call `start()` first get a
  clear error instead of dead transports.

### Fixed

- **`pause()` now stops a transport sending, not just its fallback timer.**
  Every transport drives sends from two places: a timer, which `pause()`
  cancelled, and the callback the core makes whenever it has something to send
  — which is the *primary* path. Nostr, Reticulum and Wi-Fi Direct cancelled
  only the timer, on both platforms, so a backgrounded app went on draining a
  full batch per callback: a global-mutex acquisition per message, plus a TCP
  write on Reticulum.

  The reconnect edge was the worse half, because it was durable rather than
  transient. A relay or daemon that dropped and reconnected during a background
  stay ran the connected branch, which restarted the timers unconditionally —
  so the 100ms Nostr poll (and its 30s ping) came back for the rest of the
  stay, against a transport the app had paused. The internet transport has
  always guarded that branch; the other three now do too, along with the
  callback, and both clear on `resume()` and on an explicit `start()`. Nothing
  is stranded: messages stay queued in the core, and `resume()` drains them —
  Wi-Fi Direct explicitly, since its fallback would otherwise trickle a backlog
  out at one message every two seconds.

  On iOS the arming itself is what refuses: `startMessagePolling` and
  `startPingTimer` check the flag and install the timer in one locked step,
  because there — unlike Android, where `pause()` and the reconnect edge share
  a thread — the two run on unrelated queues, and a check at the call site
  could be overtaken by a whole `pause()` and arm a fresh timer against a
  paused transport for the rest of the background stay. The poll and ping
  handlers re-read the flag as well, since cancelling a timer cannot reach a
  tick already dispatched.

- **The iOS bridge now pauses and resumes the Wi-Fi Direct manager.** It held
  one and drove its full lifecycle everywhere else, but omitted it from the
  pause/resume fan-out that covers the other four transports — so a
  backgrounded app went on browsing for peers over MultipeerConnectivity, and
  the manager's pause handling could never engage. The Android bridge already
  paused all five.

  Two of the three transports are live today. `WifiDirectTransport` is not
  registered by the bindings layer, so its managers cannot resolve a transport
  to drain and the send-path half of the fix there is forward-looking; the
  send behaviour this changes in a shipped app is Nostr's and Reticulum's. The
  iOS browsing leak above is live now.

- **The BLE path no longer runs protocol calls on the app's main thread.** On
  iOS, every CoreBluetooth delegate is a main-queue callback and the fragment
  stores made main-queue readers wait behind UniFFI calls holding the core's
  global mutex — the top App Hang cluster reported against 0.20.1. Protocol
  calls now route through the protocol queue and the fragment stores are
  lock-guarded, so a read never waits behind MLS work. On Android, a peer
  permanently unable to accept writes kept the fragment drain reposting at
  20Hz on the main thread, contending with the 100ms process tick for the same
  mutex — the reported ANR shape. The retry now backs off 50ms→2s and stops
  re-arming after ~15s of failure (the 2s polling floor still flushes the
  queue, so no fragment is abandoned), and the process tick runs with a fixed
  delay rather than a fixed rate, so an overrunning tick no longer holds the
  mutex back-to-back.

- **The remaining transports no longer run protocol calls on the app's main
  thread.** Completing what the BLE fixes started: on Android all four
  non-BLE transport managers took their ordering from the app's main looper,
  so every call into the core — which serialises on one global mutex held
  across MLS work and AndroidKeyStore access — was charged to the thread
  Android watches for ANRs. The relay transport was the worst of them, at
  10Hz for any connected session plus a keystore-backed signing burst on
  every authenticate; Wi-Fi Direct added an unbounded drain and its
  framework callbacks; Nostr and Reticulum polled off main already but kept
  their lifecycle and connect/disconnect status calls on it. Each manager now
  confines itself to a private looper, and Wi-Fi Direct's drain sends in
  batches rather than in one pass — that looper now also carries the P2P
  framework callbacks, and a broadcast that waits out its dispatch budget is
  an ANR wherever its receiver runs. On iOS only Reticulum was still
  affected, and its connected-edge status call is the expensive one — it
  flushes the entire outbox under the mutex — so both edges move to the
  queue the rest of that file already uses.

  Two lifecycle waits changed shape as part of this. A background caller of
  `stop()` is no longer cut off by a timeout (it is the caller that needs the
  stop to have finished), while a main-thread caller now gives up rather than
  parking behind the mutex. `internetForceReconnect` no longer waits at all;
  it already resolved "accepted", not "reconnected". `pause()` and `resume()`
  went the other way — on Nostr and Reticulum they used to hand the work to a
  handler and return, which paused nothing, so the core could pause underneath
  a transport still calling into it. Both now wait, and the module's pause and
  resume run their fan-out the way the stop paths already do: every transport
  is still paused (or resumed) even if one of them throws, and the first
  failure is reported once the rest are done rather than skipping them.

  Also fixed while in these files: iOS `stop()` and `destroy()` never stopped
  the Wi-Fi Direct manager, leaving a live send path holding a protocol
  instance the caller had released; that manager's session and peer map were
  read and written from three threads without synchronisation; and its send
  drain, which is deliberately unbounded, now re-reads the transport state each
  time round rather than only before it starts — a `stop()` landing mid-drain
  left the rest of the queue being fetched under the core's mutex and dropped
  for want of a session.

- **Stopping the Reticulum or Nostr transport while it was still connecting
  could leave it permanently wedged, on both platforms.** All four managers
  announce a new connection from a posted block, so a `stop()` could land in
  between. The announcement then ran against a stopped transport: it put the
  state back to `RUNNING` and told the core the transport was up moments after
  it had been told it was down — with nothing left that would ever tear it down
  again, and the next `start()` failing with `AlreadyRunning`. All four now
  check for the stop before announcing and close the connection they opened
  instead. The relay transport already guarded its posted blocks this way on
  both platforms.

  The two platforms need different amounts of machinery for it. On Android the
  gate, the state write, the status call and `stop()` all run on the one
  transport thread, so a plain check is already atomic against a teardown. On
  iOS they are spread over three queues, so a check followed by a write is two
  steps with a `stop()` free to land between them: the announcement claims
  `.running` in one operation instead, and the status call — which on Reticulum
  is a further queue hop away, the wider of the two windows — is ordered
  against `stop()`'s own call by a lock rather than by proximity to a check.
  Without that a torn-down transport could still be reported to the core as
  connected, and the core would route to a transport that never drains. The
  announcement also refuses once the connection it was announcing has died,
  which no teardown check can see: a link that opens and immediately fails
  reports itself down over a shorter path than the announcement travels, so
  the two would otherwise reach the core in the wrong order.

- **A Reticulum daemon that was unreachable or had stopped reading could stall
  `stop()`.** Its connect (up to 60s) and its TCP writes (no timeout at all)
  shared the thread that lifecycle calls wait on, so tearing the transport down
  queued behind them — and with it the React Native bridge thread. The blocking
  socket work now has its own thread, and `stop()` closes the socket rather
  than waiting for whatever is on it.

- **Android: a Reticulum connect that failed never retried.** The failure path
  cleared its own in-flight flag before handing off, and the handler that
  receives the failure decides whether to react by reading exactly that flag —
  so it saw nothing to do and returned before scheduling anything. The 1s→30s
  reconnect ladder was therefore dead for the case it exists for: a daemon
  that is not running. The transport sat in `STARTING`, where `start()` throws
  `AlreadyRunning`, until the app stopped and started it by hand. iOS was
  unaffected. Configuration set by `configure()` is also now published safely
  to the threads that read it, so a mid-session reconfigure cannot leave a
  reconnect dialling the previous host or relay set.

- **Android: stopping and restarting Reticulum while it was connecting could
  leave a second connection live and unowned.** Tearing down clears the
  in-flight flag while the connect is still blocked inside the socket call, so
  the restart opened a second connection without the first being closed.
  Whichever lost the race was left with a live reader that still believed it
  was connected — and when it eventually failed it tore down the connection
  that had replaced it. Attempts are now versioned, so one that outlives its
  session closes what it opened instead of publishing it.

  The version is checked everywhere a retired attempt could still act, not
  only before it publishes. Publishing the connection, claiming the connected
  flags and announcing the connection are three steps with a scheduling gap
  after each, and the stopped-transport check that guards the announcement is
  blind to precisely this case — after a stop *and* a restart the transport is
  legitimately starting again, so the check passes and the announcement
  reports a connection the teardown had already closed. The core would route
  to a transport with no socket, and the send loop that announcement starts
  drained the outbox straight into send failures until the next attempt
  resolved. Recoverable, since the following attempt corrects the status and
  the core retries the sends, but self-inflicted.

  Two of those checks sit inside the lock that owns what they guard, rather
  than beside it, because a check next to the write it protects is still a
  check followed by an act on another thread. The consequential one was the
  connected flags: a teardown landing between the check and the write was
  simply overwritten, leaving the transport marked connected against a socket
  that teardown had already closed — and since a connect refuses to start while
  that flag is set, the next `start()` returned having done nothing until the
  orphaned reader errored and the reconnect ladder picked it up.

  Tearing down now also ends a connect that has nothing to publish yet. Only
  closing the socket ends a blocked connect — it ignores interruption — and the
  thread it runs on is shared and outlives the session, so an attempt left
  running against an unreachable daemon held the *next* session's connect
  behind it for the rest of the 60s timeout. The per-session thread this
  replaced never had that coupling: it was abandoned mid-call and the restart
  got a fresh one.

  The version guards the teardown as well as the connect, which is the half
  with a permanent consequence. Every way a disconnect gets *observed* — a
  failed connect, a dead receive loop, a send budget exhausted — is noticed on
  one thread and handled on another, so each can arrive after the session it
  describes is over. The handler's own duplicate check could not tell: it asks
  whether *a* connection is live, never whether it is *that* one, so after a
  stop and a restart it read the new session's flags and passed. The stale
  report then marked a healthy connection down, told the core so, and started
  a reconnect ladder against it — while the connection that was actually live
  was left open with its reader parked on a socket nothing would ever close,
  leaking a thread and a descriptor for the life of the process.

- **Android: Wi-Fi Direct teardown could strand port 8988, and every session
  leaked a framework channel.** A `stop()` racing the server socket's bind
  closed a still-null field; the bind then completed, the accept loop saw the
  stop and exited, and the freshly bound socket leaked with the port still
  held — the next `start()` failed with `BindException`. The accept task now
  closes the socket it bound on every exit, and the field is published safely
  across the two threads that touch it. The accept loop also stops when *its
  own* socket closes rather than only when the transport flag clears: a
  restart that set the flag back while the old task was still waking from the
  close left it spinning at full tilt on a dead socket, emitting a diagnostic
  per turn. Separately, each `start()` created a `WifiP2pManager` channel that
  nothing ever released; `stop()` now closes it on API 27+. All pre-existing,
  and low real-world impact while this transport stays unregistered.

  Two smaller ones in the same file: the P2P state-change broadcast no longer
  makes its protocol call inside the receiver's dispatch window (it hands it to
  the same queue and returns, which is what the send drain's batch budget is
  for), and `resume()` clears the previous poll before posting one, so a resume
  that does not follow a pause no longer leaves two polling loops running.

- **Android: Nostr's reconnect bookkeeping was three plain maps crossed by two
  threads.** OkHttp's reader thread reset a relay's attempt counter and delay
  on every successful connect while the transport thread structurally modified
  the same maps scheduling reconnects — a plain `HashMap` read racing a resize
  is the classic corruption case. All three are `ConcurrentHashMap` now. Also
  fixed while there: `stop()` never shut down the OkHttp dispatcher (the relay
  manager already did), so its threads outlived every stop/start cycle.

- **Nostr could report itself connected with no relay connected, on both
  platforms.** Deciding whether the relay set had just come up sampled "is any
  relay connected" and published the answer as two separate steps, so two
  relays transitioning at once could interleave and let the later writer
  publish the earlier reader's stale answer. The core was then told the
  transport was up against an empty relay set, and every message the send loop
  drained came straight back as a send failure until a relay genuinely
  reconnected. Both halves now happen under one lock, so the last writer always
  reflects the final state. Pre-existing on both platforms.

- **A relay-delivered group message could be lost silently while the sender saw
  it delivered.** The relay's group path names senders by relay-account
  username, while the MLS credential inside the ciphertext is the sender's
  `off1…` address — so under the new identity every group frame the relay
  fanned out failed the wire-sender/credential match after decryption and was
  rejected. The rejection alone would have been recoverable (the delivery
  report's per-member backstop re-sends a copy), but the relay-path handler had
  already marked the message's logical id as seen before decrypting, so the
  backstop copy was absorbed as an already-delivered duplicate and ACKed —
  delivered exactly nowhere, with the sender told otherwise. Both the arrival
  handler and the deferred-decrypt drain now unmark the id on every arm that
  neither delivered, nor buffered, nor consumed MLS state (identity rejection,
  decrypt refusal, and the plaintext-spoof drop, whose id is attacker-chosen
  wire input), so the backstop copy is processed honestly: it buffers un-ACKed
  and the sender keeps custody instead of receiving a false delivery ACK. The
  drain half matters as much as the arrival half — a relay copy can outrun its
  Welcome, in which case it is buffered before any decrypt and its
  mis-attribution is judged on the drain instead, an ordering a misbehaving
  relay can simply choose. Preventing the loss
  outright is the job of the `group_delivery_v3` capability gate below, which
  keeps a mis-attributed relay copy from existing (and spending the
  ciphertext's one decryption) in the first place.

- **The relay group control plane no longer recognized this device, so it added
  itself to its own groups and never left them.** The React Native bridges tell
  their relay adapter who "self" is with the `profile`, and compare that against
  ids that arrive from two different places: group rosters and leave
  notifications come from the core and name members by `off1…` address, while
  the relay's own role-change answer names accounts by their relay username.
  Since the identity change the profile matched neither reliably, so all three
  self-checks were dead. The SDK sent the relay an `AddGroupMember` naming its
  own address; leaving a group never sent the relay-native `LeaveGroup`, so the
  relay kept fanning that group out to a device that had left; and being
  promoted to admin mid-connection never re-enabled the membership sync an
  earlier denial had suppressed. The adapter now recognizes itself by profile
  *or* address, resolved fresh on each use so it works across MLS
  initialization and identity rebuilds, and the role-change answer is read from
  the field the relay actually sends. The same two-namespace fix is applied to
  the presence self-filters, which could otherwise put this device in its own
  presence watch set. No configuration, and nothing for an app to call.

- **No MLS session could be established over the relay, and `off1…` recipients
  were unreachable through it.** The relay knows a connection by the account
  name it authenticated, and stamps that name on every frame it forwards. Since
  the identity change the core stamps `Message.sender` with the derived address,
  so the receiver's transport-identity check compared an address against an
  account name and rejected the frame — which took `__MLS_KEY_PKG__` and
  `__MLS_WELCOME__` with it, while already-established sessions kept decrypting
  (`__MLS_ENC__` is data-plane and ungated). Outbound was broken in the same
  place from the other side: an `off1…` recipient did not exist in a registry
  keyed by account name. Both bridges now prove their address to the relay
  immediately after authenticating, by signing a per-connection challenge from
  the relay's `Authenticated` frame under a dedicated domain, and the relay
  routes and attributes that connection by address from there on. Requires a
  relay advertising the `address_routing_v1` capability; against an older one
  the bridges stay silent and the connection behaves exactly as before. No
  configuration, and nothing for an app to call.

- **Transport callbacks registered before `initialize_mls` were silently
  dropped.** `set_*_transport_callback` installed onto whichever transport was
  registered at the time, and the identity rebuild replaces those objects — so
  the BLE and Reticulum callbacks both bridges wire during `create()` died with
  the objects that held them, and the Nostr one never landed at all, because
  Nostr has no transport until the rebuild builds it. Callbacks are now
  retained and re-applied after the rebuild. This was masked by the platform
  polling loops and cost latency rather than messages (≤100 ms for Nostr, up to
  5 s for Reticulum), but the Android log claimed "event-driven sending active"
  in every case, including for Wi-Fi Direct, whose transport is not registered
  at all. Those logs now say what is true, and the Wi-Fi Direct setter warns
  that it is inert. **The polling loops remain load-bearing — do not remove one
  on the strength of a callback being registered.**
- `getTopology().local_user_id` reported the `profile` rather than this
  device's address, for the lifetime of the instance. Both bridges synthesized
  the field from config instead of reading `localAddress()`, which handed an
  app a local storage selector under an identity-shaped key. It is now the
  address, and empty before `initialize_mls` derives one.
- `listSessions` no longer reports a peer for a session slot that names two
  other parties; it requires one half of the slot to be this device.
- React Native: `localAddress()` no longer serves a destroyed instance's
  address. The cache is cleared by `destroy()` and populated eagerly by
  `start()`, so the documented `destroy` → `wipePersistedState` → `start`
  cleanup reports the newly minted identity rather than the dead one, and
  session attribution cannot run against an empty cache — which previously
  named *this* device as the remote peer.

### Documentation

- **Three privacy claims the code does not support are corrected.**
  Documentation only; no behaviour change. Each is the kind of line a skeptical
  reviewer tests first, and each was testable and wrong.

  **"No routing tag has a computable private half" was false.** The tag is
  `SHA-256(address)` → scalar → x-only pubkey, so anyone holding an address can
  reconstruct the entire keypair behind that address's tag; the same sentence
  also described both derivations as domain-separated when only the seal key
  is. The property the addressing migration actually bought is narrower and is
  now what gets claimed: nothing seals to the tag any more, so reconstructing
  one decrypts nothing. Recorded on `routing_tag_for_address` itself, since
  that is where the derivation is read.

  **The gift-wrap anonymity set is not "every NIP-17 conversation on the
  relay".** Reusing kind `1059` defeats a scrape *by kind*; it does not make a
  tag anonymous to a relay you subscribe on. Three distinguishers are now
  enumerated on `NOSTR_GIFT_WRAP_KIND` and under `docs/nostr.md` §"What a relay
  can see" — the client's own `REQ` names its routing tag to every relay it
  connects to (decisive, and unavoidable while a recipient subscribes on a
  stable tag); requesting legacy kind `4` beside `1059` under one `#p` is a
  shape a NIP-17-only client does not have; and kind-`30443` records sit at our
  own tag signed by the install's real Nostr key, which a relay can join to it.
  None of the three reveals content: who is talking to whom, about what, and
  under which app id all stay sealed.

  **The 160-bit second-preimage figure now names its collision bound.** ~2^160
  is the cost of aiming at an address that already exists. The birthday bound on
  the same truncation is ~2^80, which buys two identity keys under one address
  rather than a chosen peer's — enough to equivocate, and enough to defeat the
  "one identity cannot hold two leaves" property the MLS leaf binding otherwise
  inherits from MLS signature-key uniqueness. Stated on `Address::HASH_LEN` as
  the deliberate trade against BLE frame budget that it is, rather than left for
  reviewers to derive.

- **`SECURITY.md` no longer invites reports against a mechanism that was
  deleted.** Its scope list asked for "TOFU key management bypasses"; the pin
  store is removed in this release, so the entry now names the sender-address
  derivation gate that replaced it, in both its control-frame and MLS-leaf
  shapes. `CLAUDE.md` likewise stops describing the control gate as
  "Ed25519+TOFU".

- **The internal downstream codename is removed from shipped artifacts.** It
  remained in two npm-shipped Swift files, in the historical cleartext envelope
  reproduced here and in `docs/nostr.md`, and as the `AppId` fixture in the
  transport's sealed-payload leak test.

### CI/CD

- **A prerelease tag no longer takes over the npm `latest` dist-tag.** `npm
  publish` defaults every upload to `latest`, so a `vX.Y.Z-rc.N` tag — or a
  branch dispatch, which falls back to `0.0.0-dev` — became what a plain `npm
  install @offline-protocol/mesh-sdk` resolved to. Prerelease versions now
  publish under `next`, derived from the resolved version rather than the ref so
  a hyphenated branch name cannot trip it. This matters more than it used to:
  crates.io publishing treats an rc tag as a full rehearsal, which makes cutting
  one a routine thing to do rather than a rarity.
- **Every third-party action in `release.yml` is pinned to a commit SHA.** The
  workflow mints the provenance attestations consumers are told to trust, so
  its supply chain no longer floats on mutable version tags.
- **`cargo doc` is gated in CI.** A new `Rustdoc` job builds the workspace under
  `RUSTDOCFLAGS: -D warnings`. It caught six broken intra-doc links: four public
  items linking to private ones — which resolve to nothing for every reader not
  building the crate themselves — and two simply wrong paths. Deliberately
  *without* `--document-private-items`, which would have suppressed four of the
  six rather than surfaced them.


## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
