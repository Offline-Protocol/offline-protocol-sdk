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

### Fixed

- **A resend no longer queues behind its own first copy.** A frame whose
  earlier copy is still waiting in a local carrier queue has not gone out, so
  nothing can have answered it, and the retry ladder resent it anyway. On a
  phone chain with a slow Bluetooth link, its 1 s and 3 s resends queued
  behind the first copies, and the receipts for the sender's other messages
  waited behind them: ten copies ahead of the receipt that completed a round
  in a fast round, twenty in a slow one. The ladder now holds such a resend
  for one more rung, and only within the minute from the last send that
  already bounds the acknowledgement timeout's extension, so a link that
  died without telling the transport still cannot pin a message to it.

- **The browser hands its Bluetooth link a few fragments at a time.** The
  worker passed every fragment to the page at once, into one write queue per
  link, so the engine saw nothing queued: it could neither put a receipt
  ahead of data nor hold a resend behind its first copy. It now keeps at most
  four unwritten fragments per link with the page, and a Bluetooth sink's
  `write` may return a promise that settles when the write does, which paces
  the node. A sink that returns nothing paces nothing, as before. In one run
  of ten rounds on the phone chain, with the fix above and #706, receipts
  over 5 s fell from 68 of 200 to 15, each of the 15 a receipt the next hop
  refused under its rate limit.

- **A device hands its own frame to every neighbor a phone can hold.** A
  forward's fan-out of three covers every link of a four-link phone but the
  one the frame came from, and a device's own frame was given the same
  three, so one of its four links was left out, chosen by the message id. A
  message whose only way on was that link never left, nor did any resend of
  it, and a delivery receipt was lost the same way when the link left out
  was the one the message arrived on. A device's own frames now go to one
  more neighbor than a forward.

- **A delivery carried across the mesh is answered once, not once per path.**
  A recipient answered every copy it heard, and one delivery arrives by as
  many paths as neighbors carried it, so each message cost up to four answer
  floods in a crowd of phones holding four links each. A copy a relay carried
  is no longer answered when the same message, under the same resend
  generation, was answered in the last five seconds. A copy from the sender
  itself, one over any other carrier, and a resend under a new generation are
  still always answered. In a simulated crowd of forty phones this cut the receipt frames
  by a third to two thirds and the total airtime by up to a half.

- **A resend crosses the mesh as far as the first copy did.** Every device
  that carries a frame ignores its id for ten minutes, so a sender's resend
  stopped at the first forwarder: the device beside the sender carried it
  again, but to every device after it the copy was an echo. A message lost
  past the first forwarder could not be recovered by retrying, and neither
  could a lost delivery receipt, since the recipient answers only a copy
  that reaches it. A resend that reaches a neighbor a full acknowledgement
  timeout or more after the last generation did now moves to the next generation
  (at most three, and time out of range does not count), carried in a new reserved
  metadata key, `__resend`, and a forwarder suppresses each generation
  separately, so it is a new flood whose own copies are still suppressed.
  Earlier resends stay copies of the first flood, so a receipt that is merely
  slow costs nothing extra; a message stays at the last generation for as long
  as it lives, so one for a recipient who is gone crosses the mesh at most once
  per ten minutes from then on, as it did before. In a simulated crowd of forty
  phones, every delivery receipt now comes back; at one hand-off in ten lost, every message
  arrived and more than nine in ten were acknowledged within 35 seconds, where
  a message or receipt lost past the first forwarder used to stay lost. Older
  senders send no stamp and behave as before.

- **A forward that runs out of send budget partway through its fan-out is
  finished when the budget refills.** The device counted it forwarded at the
  cut, so the frame went to whichever neighbor came first, chosen by a hash
  rather than by where its recipient is, and its id stayed suppressed for ten
  minutes. In a simulated crowd of forty phones with six conversations at
  once, the delivery receipts outran the budgets this way and 4 to 7 of about
  20 never came back; this fix alone brought that to 0 to 2, and the resend fix
  above the rest. The remaining neighbors
  now get the frame within the few seconds a queued forward is kept, and
  only those chosen in the first place, so the fan-out is never widened.

- **A mesh-only device stops flooding its neighbors with probes for peers that
  are gone.** A pending 1:1 session was probed every five seconds forever when
  no relay was there to say its peer was unreachable, and pending sessions are
  restored at every start, so a phone that had met a few devices (or earlier
  installs of the same app) kept probing all of them. Each probe was offered to
  every neighbor on every resend: on two phones over Bluetooth, eight such
  peers held the only link for minutes and a service subscription's reply
  waited behind them. A probe that only the mesh could carry now backs off on
  the relay verdict's 15 s to 600 s ladder, and discovering the peer probes it
  again at once.

- **A Python service with phones on its Bluetooth peripheral reaches the
  engines behind its peer stream.** The peripheral announced each subscribed
  central under its Bluetooth id as well as under the address its hello
  proved, so every phone was two Bluetooth peers in the core. The mesh fan-out
  ranked links by quality alone, and a peer stream, which measures no signal,
  scores mid-scale: with two phones linked, the four Bluetooth links took all
  three slots on every send, and a message for an engine two hops away was
  never delivered, on any retry. The peripheral now announces a central only
  under its proven address, and the fan-out gives the strongest link on each
  carrier a slot before any carrier gets a second, so three phones and one
  peer stream no longer starve the stream either.

## [0.32.0] — 2026-11-05

> **A service can be subscribed to.** A consumer takes a lease on a topic of a
> provider's service, or on every topic matching a pattern, and receives each
> update as it is published, sealed in the 1:1 MLS session. A topic keeps the
> current value per key or every event, and a consumer that missed updates
> gets the state back at its next renewal. The calls are on `MeshServices` in
> every binding, in React Native and Flutter, and in the browser.
>
> **A C# binding (preview).** `bindings/csharp` drives the native library
> through the C API, with typed config, storage providers, events, telemetry
> and transport callbacks. It ships as source, not on NuGet.
>
> **Bluetooth LE holds up in a pocket.** A backgrounded iPhone keeps its
> links, an Android phone with its screen off keeps discovering peers, an
> iPhone with Bluetooth off no longer reports it up, and an Android phone keeps
> talking to an iPhone whose app restarted. Frames for a Bluetooth neighbor no
> longer leave through a relay that cannot reach it.
>
> **Rust code that names every case or every field breaks in two places.**
> `Event` gains `ServiceSubscribeReceived`, `ServiceSubscriptionChanged` and
> `ServiceUpdateReceived`, and `ProtocolConfig` gains `service_subscriptions`.
> An exhaustive TypeScript `switch` over the event union needs the three
> `service_*` cases, and a hand-written `MeshServices` conformer needs the
> twelve new methods. Nothing is removed from any binding; what behaves
> differently is in
> [§30](docs/UPGRADING.md#30-behaviour-that-changes-without-a-compile-error-v0320).

### Added

- **`__MEMBER__` is reserved, and the application membership layouts exist.**
  The control frame of the membership exchange
  ([ADR 0037](docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md))
  is reserved and consumed on receipt, never delivered as text or answered,
  ahead of the exchange itself. A build that does not reserve it rejects the
  frame with a security warning, or shows it as text when `require_encryption`
  is off, so this release is the floor for sharing a radio neighbourhood
  cleanly. `offline-protocol-sealed` gains `membership`: the certificate, the
  two exchange frames and the end-to-end proof, each at one fixed size, with
  their signing payloads and key derivation inputs, pinned by
  `app-membership-v1.vectors.json`
  ([control messages](docs/spec/control-messages.md#application-membership)).
  The end-to-end proof's sealing key has its own label,
  `offline-app-member-e2e-key-v1`, apart from the signing domain. No node
  sends the frame: the exchange itself is held back, and ADR 0037 is parked.

- **A browser node's start can be aborted.** `OfflineNode.start({ signal })`
  ends the worker of a start that has not finished, and with it the
  account's lock or its place in the queue for one, and rejects with the
  signal's reason. A page that gives up on a start no longer holds the
  account while that start replays the logs
  ([W15](docs/bridges/web.md#w15-a-page-that-loads-offline-caches-one-build-whole)).

- **The lobby loads with no network.** Opened once online, the `web-lobby`
  example loads offline, and carries a manifest to install it as an app
  (not yet run on a phone, which is step 9 of the two-device checklist): a
  service worker caches the page, the SDK and the engine module as one
  build, named by an id `scripts/build-id.sh` writes, and a new build
  takes over only when a page asks, whereupon every page with a running
  node stops it and reloads. The engine's worker starts from inside the
  service worker's scope, or offline its script comes from the network.
  The SDK ships no service worker; the lobby's is the worked example, and
  the browser run reloads it offline in three engines
  ([W15](docs/bridges/web.md#w15-a-page-that-loads-offline-caches-one-build-whole)).

- **C# / .NET binding scaffold.** `bindings/csharp` loads `offline_protocol_uniffi`
  through the C API, runs engine calls on a dedicated thread, and ships a
  consumer check for create → file-store MLS → start/stop
  ([docs/bridges/csharp.md](docs/bridges/csharp.md)).
- **C# storage providers, events and telemetry.** Custom storage plugs in
  through `IProtocolStorage` and a single-use `NativeStorageProvider`, with
  `RunStorageConformanceAsync` to verify a backend and an in-memory
  `MemoryProtocolStorage`. Events arrive on a dedicated delivery thread, where
  a throwing handler is counted instead of ending the process, or through
  `PollEventAsync`. `EnableTelemetryAsync` lets the library fill `os` and the
  binding fill `os_major`, and the lifecycle calls follow C12
  ([docs/bridges/csharp.md](docs/bridges/csharp.md)).
- **C# typed config and generated local API.** Every engine method has a
  typed wrapper on `ProtocolEngine`, `engine.Services` or `engine.Data`, and
  `ProtocolEngine.CreateAsync` takes a `ProtocolConfig` record that leaves an
  unset optional field to the engine's own default. Both are generated from the
  interface definition, so a new method or config field reaches C# when the
  generator is rerun ([docs/bridges/csharp.md](docs/bridges/csharp.md)).
- **C# transport callbacks.** `SetBleTransportCallbackAsync`,
  `SetWifiDirectTransportCallbackAsync`, `SetReticulumTransportCallbackAsync`
  and `SetNostrTransportCallbackAsync` install
  `offline_protocol_set_transport_callback`. Notifies coalesce per carrier and
  reach app code on one delivery thread per engine, behind a bounded queue
  whose rejections `DroppedTransportNotifyCount` reports. A throwing handler
  is counted in `TransportHandlerFailureCount` and never ends the process
  ([docs/bridges/csharp.md](docs/bridges/csharp.md#cs7-transport-callbacks)).
- **A service can be subscribed to.** `subscribe_service` gives a consumer a
  lease on one topic of a provider's service, and the provider's
  `publish_service_update` reaches every subscriber as it happens: once over a
  live link, or through the reliable ladder across hops, where a `latest`
  value replaces the one still queued for its key rather than waiting behind
  it. A topic is `latest` (current state per key) or `log` (every event, with
  a checkpoint), and a consumer that missed updates gets the state back at its
  next renewal, after a provider restart, or on `resync_service_subscription`.
  Every subscription frame is sealed in the 1:1 MLS session with no plaintext
  fallback, and none is written to the outbox's store: nothing of a
  subscription survives a restart, and a frame from before one is dropped.
  Who may subscribe is the service's policy: ask the application (the
  default), accept every peer with a session, or rules by peer and group.
  The calls are on `MeshServices` in every binding (Swift, Kotlin, Python,
  Ruby, Java, C), in React Native and Flutter on both platforms, in the
  Python services wrapper and local API, and in the browser worker, where
  `examples/web-coordination` shows devices watching each other's state. The
  decision is ADR 0035, the behaviour `docs/state-machines/service-subscriptions.md`.
- **A subscription can be narrowed.** A topic pattern (`rooms/*`, or `*`)
  follows every matching topic in its mode, up to 64, including topics first
  published after it, each caught up and recovered on its own; every update
  names its topic. A key filter limits a `latest` subscription to the keys it
  lists, exact or by prefix. The provider applies both, so a filtered key or
  an unmatched topic never leaves its device. `subscribe_service_with` takes
  the filter in Rust, and `subscribe_service` takes `keys` in every binding.
  Group-backed topics, the rest of ADR 0035's third phase, are proposed in
  ADR 0036.

### Fixed

- **An iPhone with Bluetooth off no longer tells the engine Bluetooth is up.**
  `start()` reported Bluetooth available at once and again a second later,
  whatever the radio's state. With Bluetooth off, unauthorized or unsupported,
  the Bluetooth manager reports it down, and the second report then marked it
  up again, so the engine routed sends to a radio that was off until the
  radio next changed state. Neither report is made while the radio is
  unavailable. Coming to the foreground likewise restated the Wi-Fi Direct
  layer up when that transport was disabled, stopped or had failed to start;
  it now does so only while the transport runs.

- **An event callback may replace itself, and one that throws no longer
  costs the polling queue an event.** Events the bindings raise themselves
  (neighbor discovered and lost, signal strength, carrier status) reached the
  callback with its slot locked and no isolation, unlike events from the
  engine: a callback that called `set_event_callback` deadlocked, and one
  that threw skipped the polling queue. Both kinds are now delivered the same
  way.

- **Deleting a 1:1 session drops the Welcome still waiting for it.**
  `mls_get_pending_welcome` kept returning, after the session was deleted, an
  invitation to the group that no longer existed.

- **A browser Bluetooth link-up refused before `initialize` leaves no link
  behind.** The address stayed recorded as linked, so its fragments were
  accepted and the next link-up was taken for a relink and never announced
  the peer.

- **Android `start()` with no protocol created rejects instead of putting up
  the mesh service.** It started the foreground service over no engine, which
  a sticky restart then kept alive. The JavaScript wrapper never does this; a
  direct caller of the native module could.

- **A second SDK app on an Android phone gets its own LAN streams.** The
  peer-stream listener bound the fixed port 8988, so a second app on the
  same phone failed to bind it and still advertised 8988 on the LAN: every
  peer that dialled it reached the first app, and it had no inbound streams
  at all. A listener that cannot take 8988 now binds a free port and
  advertises that one. A group's clients can only dial 8988, so an app
  without it never creates a Wi-Fi Direct group and joins instead. A start
  that cannot bind any port now fails rather than advertising one, and a
  start that fails at any point leaves nothing running.

- **Enabling Wi-Fi Direct on Android no longer starts it a second time over
  a start in progress.** `enableTransport('wifiDirect')` stopped the manager
  first only when it was running, while the Internet branch also stopped it
  when it was starting. An enable that found a start still in progress, or a
  start that had thrown partway and left the manager starting, started it
  again over what the first start had brought up: a second broadcast receiver
  over the first, and a second listener for the port the first one held. The
  Wi-Fi Direct branch now stops a starting manager as the Internet branch
  does.

- **Frames for a Bluetooth neighbor no longer leave through a relay that
  cannot reach it.** The application dispatch sent a message for a recipient
  with a live direct mesh link over that link, but retries,
  acknowledgements and session control frames went straight to DORS, which
  ranks carriers without looking at the recipient. Nostr, the internet relay
  and Reticulum accept a frame for any recipient and report success, so on a
  device with a relay up those frames for a peer whose only path was the link
  never arrived and were retried the same way until they failed. A browser
  linked over Web Bluetooth to a phone with relays configured never completed
  its session: after the Welcome, the phone's frames went to Nostr and the
  browser's dial timed out. `TransportManager::send` now applies the
  dispatch's rule on every path, and a refusal still falls through to
  selection (`send_selected`)
  ([transport lifecycle](docs/state-machines/transport-lifecycle.md#what-sent-means-at-each-layer)).

- **A backgrounded iPhone keeps its Bluetooth LE links.** About two minutes
  after the app went to the background, beside a peer whose writes kept it
  awake, the iPhone stopped sending on every link and never recovered, even
  in the foreground, while it kept receiving. The scan watchdog read the
  silence of a background scan as a stalled stack and replaced the central
  manager, and a second manager under the same restore identifier reports
  `unsupported` for good. A backgrounded iPhone now scans for the mesh
  service, the only scan iOS runs in the background, so it also discovers new
  peers there; the watchdog does nothing in the background; and a restart
  never replaces the manager
  ([C14](docs/bridges/README.md#c14-a-quiet-scan-in-a-pocket-is-not-a-stall)).

- **An Android phone with its screen off keeps discovering peers and stops
  churning its advertising address.** With the screen off, the stack pauses an
  unfiltered scan, so the phone discovered nobody, and the watchdog restarted
  the scan every 30 seconds and reset the stack every 90. Each reset restarted
  the advertisement under a new random address, which a linked peer then dials
  and never connects (ADR 0030). With the screen off the scan now filters on
  the mesh service, its silence is not a stall (it is restarted after five
  quiet minutes, never escalating to a reset, so a scan Android silently
  refused does not stay dead), a screen transition moves the scan two seconds
  later and only if it must, and the reset restarts the advertisement only as
  the forced refresh does
  ([C14](docs/bridges/README.md#c14-a-quiet-scan-in-a-pocket-is-not-a-stall)).

- **Two devices that meet only through a third can invite each other to a
  group.** When the packages of such a pair crossed, so that each built a
  session from the other's before either Welcome arrived, the side the
  tiebreak made adopt had spent the other side's only package on the group
  it gave up, and every group invite it made after that failed with no key
  package. A device in between makes the crossing common, since it forwards
  each frame after its own jitter: two browsers linked to one phone hit it
  in about one run in three on the host. The side that keeps its group now
  sends a fresh package for each session Welcome it does not adopt
  ([both-create convergence](docs/state-machines/session-lifecycle.md#both-create-convergence)).

- **An Android phone keeps talking to an iPhone whose app restarted.** An
  iPhone keeps its Bluetooth LE links across an app restart, and its new
  process knows nothing about them: not the hello that named the Android
  phone, and not where it was subscribed. Android saw no disconnect and
  carried nothing more, in one direction or both, until its own app restarted.
  Android now sends over any ready client link to the peer, not only the
  address the peer's hello mapped. A dial that proves who writes to its
  server, and is then refused as a duplicate link, now maps that writer before
  it closes. And a client link that keeps taking writes and has never had an
  answer is dropped and redialled, which writes the hello again, about 20 to
  25 seconds after its first write, even when the peer holds another link. A
  link the peer has answered once, even before it restarted, is never dropped
  for silence, and a peer is redialled once until it is heard from again, so
  one that only receives frames asking for no acknowledgement is not redialled
  over and over. On two phones, the first iPhone restart beside a linked
  Android phone failed before, as did one in ten restarts of both apps; after,
  36 of 36 restarts and connects carried messages, and on two earlier runs of
  the same checks a first link left dead by an iPhone relaunch was redialled
  and carried the message within 25 seconds
  ([kotlin-devices.md](docs/bridges/kotlin-devices.md),
  [K9](docs/bridges/kotlin.md#k9-a-bluetooth-le-link-outlives-the-app-on-the-other-end)).

- **`sendReadReceipt` works again from React Native on Android.** Since the
  shared engine bridge (0.31.0), every array argument crossed the Android
  adapter as integers, so the message ids, which are strings, threw on every
  call. Arrays now keep each element's type, through the same conversion an
  array inside a map already used, and byte arrays still reach the engine
  unchanged. Empty ids are dropped as they were before, now on iOS too, so
  both platforms and Flutter apply one rule. A guard holds every array
  parameter of the adapter to that conversion, and JVM tests pin what it
  returns.

- **An iPhone accepts and sends connection requests without a key package
  again from React Native.** Since the shared engine bridge (0.31.0), the iOS
  adapter passed an omitted `keyPackage` as a boxed nil that the argument
  reader refused, so `acceptConnectionRequest` and `sendConnectionRequest`
  rejected with "'keyPackage' must be a byte list" unless the app passed one.
  Apps that catch the promise saw two phones that never connected. An omitted
  list now reaches the engine as absent, as it always did on Android, and an
  empty list is still read as none on both platforms. A Swift package suite
  sends and accepts through the shared bridge with the key package omitted.

- **`createInvite(name, true)` signs the invite again from React Native on
  iOS**, and **`checkInternetPresence(id, { force: true })` forces the check
  again on both platforms.** Since the shared engine bridge (0.31.0), the iOS
  adapter keyed the signed flag `sign` where the bridge reads `signed`, and
  both adapters nested `force` in an `options` map the bridges never opened,
  so both were silently dropped. `tools/check_rn_adapter_argument_keys.py` now fails the
  generator gate on any key an adapter sends that its bridge never reads.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.31.x](docs/changelog/0.31.md) | 0.31.0 |
| [0.30.x](docs/changelog/0.30.md) | 0.30.0 |
| [0.29.x](docs/changelog/0.29.md) | 0.29.0 |
| [0.28.x](docs/changelog/0.28.md) | 0.28.0 |
| [0.27.x](docs/changelog/0.27.md) | 0.27.0 |
| [0.26.x](docs/changelog/0.26.md) | 0.26.0 |
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
