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


### Added

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

- **Messages sent over an explicitly chosen transport skipped the negotiated
  binary wire codec.** `send_via_transport` never stamped it, while the
  selection path did, so a peer known to support the binary codec silently fell
  back to JSON whenever a send bypassed selection. Both paths now share one
  stamping helper. Visible only as larger frames on the wire, with no error and
  no event, which is why it survived.

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

## [0.22.0] — 2026-08-18

> **The battery-aware relay policy was complete and unreachable; it now runs.**
> DORS energy scoring and the forwarding floor that stops a dying phone
> carrying other people's traffic both read a charge level that nothing in
> production ever wrote. On every real device the floor was never applied and
> forwarding always took its "unknown battery means willing" branch. The feed
> now reaches the engine, and it does nothing until your app calls
> `setBatteryState(level, isCharging)`.
>
> **Relay standing is derived from what a device has carried**, not from what it
> looks capable of. `relay_promoted` / `relay_demoted` / `isRelay()` report
> observed forwarding over a rolling window, so they need no battery feed at
> all; `relay_promoted.battery_level` is now nullable, and `isRelay()` answers
> `false` on a capable device that has had nothing to carry, including any
> device with a working internet relay. `relayThreshold` and
> `ProtocolConfigExtended` are removed along with the state machine they
> configured.
>
> **Four more breaks to check before you bump.** `RelayPriority` is spelled
> `never` / `auto` / `always`, so a `getRelayPriority()` result compared against
> `'medium'` no longer typechecks. `set_battery_level` is fallible, so direct
> Swift callers need `try`. `updateDorsConfig` and `updateRelayConfig` are true
> partial updates on both bridges, which previously rebuilt the whole config
> from literals and reset every field the payload did not name; an app that
> leaned on that to restore defaults must now pass the values explicitly. And
> `getDorsConfig()` answers from the engine rather than an FFI-local cache,
> which had been handing `preferOnline: false` to an app constructed with
> `true` and silently disabling internet-first routing on the write-back.
>
> Migration notes in
> [UPGRADING §11.3](./docs/UPGRADING.md#113-relay_promoted-and-relay_demoted-can-now-actually-fire-v0220).

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
  online device are the honest answer rather than a fault. The counters include
  `refusedQueueFull` and `abandonedOverdue`, the two that record frames
  genuinely lost under pressure: without them a device shedding traffic reports
  nothing but healthy-looking deferrals, which is the reading an app most needs
  to be able to distinguish.

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
  `InvalidState` (too many lookups in flight, retry shortly), `NotStarted` (the
  protocol is not running, retry after `start()`), and `InvalidArgument` (not a
  claimable name).

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

- **`setBatteryState(level, isCharging)` — the battery feed the DORS and
  forwarding policies were always waiting for.** See Fixed below for why this
  is the headline change rather than a convenience. `getIsCharging()` reads it
  back.
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

- **`updateDorsConfig` and `updateRelayConfig` are partial updates on both
  platform bridges**: a field the payload omits keeps its **current** value,
  read back from the live engine. Both bridges previously rebuilt the whole
  `DorsConfig` from hardcoded literals, so an update meaning to change one
  field silently reset every other one — the same defect as the three fields
  `updateDorsConfig` reset behind the caller's back, one layer up and affecting
  all eighteen. Apps that relied on `updateDorsConfig` to restore defaults must
  now pass the values explicitly.

### Fixed

- **Key-package lookups no longer end on the fastest relay's answer.** A query
  goes to every connected relay and each sends its own end-of-stored-events,
  but both bridges closed the subscription on *all* relays at the first one and
  reported the query finished there. A peer whose key package sat only on a
  slower relay was unreachable for cold contact, and a relay holding nothing
  won that race by having nothing to send. Each relay's subscription now closes
  as that relay finishes, the query completes when every relay it asked has
  answered, and a silent relay is bounded by a 10s bridge deadline, kept under
  the engine's 30s sweep so the bridge stays the one that decides: it is the
  layer that knows which relays actually replied. That deadline is measured on
  a monotonic, sleep-inclusive clock on both platforms. iOS measured it with
  `Date()`, so a clock correction expired live queries early and completed them
  from whatever subset had answered, which is the outcome waiting for every
  relay exists to prevent. Username resolution rides the same machinery. The
  cost is latency: a lookup now takes as long as its slowest relay rather than
  its fastest.

- **`setRelayPriority` no longer discards a concurrent relay-config update.** It
  read the config, changed one field and wrote it back across two separate lock
  acquisitions, so an `updateRelayConfig` that landed in the gap was reverted by
  the stale copy. The read and the write now happen under one lock.

- **A replayed group frame refused on identity grounds no longer earns a
  delivery acknowledgement.** The mesh handler marks a message identifier
  before attempting the decrypt, which bounds replay amplification to one
  crypto operation per identifier, but it never released that mark when the
  decrypt came back a security refusal (the wire sender and the
  MLS-authenticated author disagree). The refusal withholds the ACK and clears
  the transport-level mark, so a replayed copy reached the group handler
  again, found its identifier marked but not pending, and was classified as
  already delivered, which acknowledges it. Withholding that acknowledgement
  from whoever forged the attribution is the entire point of refusing
  silently: an ACK confirms the device is online and processing. The handler
  now releases the identifier on refusal, matching the relay path and the
  drain, at a cost of one crypto operation per replayed copy.

- **`resolveUsername()` no longer promises an event the engine cannot send.**
  It gated only on the discovery switches, which survive `stop()`, while the
  event it promises is emitted from `process()`, which is inert unless the
  protocol is running. A lookup requested on a stopped or paused instance
  therefore returned `true` ("an answer is coming") with nothing able to
  deliver one, and — the part that made it permanent — left its registration
  behind, so every later attempt at that name answered `false`, which says the
  same thing again. The caller was told twice that a resolution was in flight
  and had no way to learn otherwise. It now rejects with `NotStarted`, after
  the discovery checks, so an app with discovery switched off still learns
  that first rather than being sent round a start-and-retry loop to find out.

  `stop()` answers the lookups it is about to strand rather than dropping
  them: a resolution that heard from some relays emits what it has, and one
  that never reached a relay emits the empty set, both on the terms the
  deadline sweep already uses. Discarding them silently would leave the
  promise broken and the registrations standing.

- **`getDorsConfig()` reports the engine's configuration, not a copy of the
  last thing written through the FFI.** It answered from an FFI-local cache
  that nothing populated until the first `updateDorsConfig`, falling back to
  the core defaults until then — and `preferOnline` is the field whose default
  (`false`) disagrees with what construction takes from `ProtocolConfig`. An
  app created with `preferOnline: true` that then performed the documented
  read-modify-write to change *any other* field read `false`, wrote it back,
  and silently turned internet-first routing off. React Native was shielded
  only by accident (its TypeScript layer sources the flag from the `dors`
  section, whose create-time application populated the cache), so this bit
  Swift, Kotlin and Python callers. The getter now reads the live selector,
  the way `getMeshRelayTunables()` reads the governor, and the cache is gone
  rather than corrected: a second copy of a value is a thing that can disagree
  with it.

- **Every mesh forwarding dial that could silently switch forwarding off is
  now refused at construction.** These share one failure and it is the worst
  kind: the device keeps running, reports no error, and carries nothing, which
  is indistinguishable from a quiet neighborhood. Each is also the value a
  caller reaches for meaning "be conservative". Refused now: a `maxTtl` or
  `denseMaxTtl` of zero (every arriving hop budget clamps to nothing, so frames
  are refused before being queued, and the dense ceiling fails only in a
  crowded room, where the mesh matters most); a `queueCapacity` of zero (every
  admission rejected as queue-full); a `ratePerSec`, `burst`, `peerRatePerSec`
  or `peerBurst` that is zero, negative or `NaN` (the token buckets clamp their
  inputs to zero, so the bucket never releases a token — `NaN` matters on its
  own, since `f32::max` returns the other operand for it and it therefore
  arrives as zero rather than failing anywhere visible); and a `jitterMin`
  above `jitterMax`, which collapsed the delay spread to a single millisecond
  and, because the overdue check reads `jitterMax`, let a ten-minute minimum
  validate clean while abandoning every forward. Two more are refused for the
  neighbouring failure, a device that forwards but never settles a relay
  standing: a `biasMinScale` outside `(0.0, 1.0]` (zero or negative inverts the
  ramp, above one turns the bias into a bonus for the weakest device), and a
  zero `activityWindow`, `activityMinForwards` or `activityIdleWindows`, which
  leave `relay_promoted` and `relay_demoted` either chattering or silent. This
  matters more now that the section is settable from JavaScript: these were
  builder-only before.

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
  DORS energy scoring skipped its battery term and message forwarding always
  took its "unknown battery means willing" branch, meaning the floor that stops
  a device spending its last few percent carrying other people's traffic was
  never applied. The relay role read the same empty map and could not promote
  either, which is why `QUICKSTART.md` documented events that could never
  arrive; that half is fixed differently, because the role is no longer derived
  from battery at all (see Changed). The host feed now reaches the engine, so
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

### Documentation

- **Every publishable crate ships a README.** The eight workspace crates
  published to crates.io rendered an empty page, because cargo picks up a
  crate-root `README.md` and none of them had one. Each now carries its own, and
  a CI check refuses a new publishable crate that arrives without one.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
