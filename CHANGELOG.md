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

### Added

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
  uploader with idempotent retry: 1 s doubling to 15 min with jitter,
  `Retry-After` honoured, a six-day expiry inside the ingest's deduplication
  window, a halt on 401 and 403, deferral below 15% battery unless charging,
  and one final flush of up to three seconds on disable, background and end
  of session. The transport-availability edge the tick already computes wakes
  it when the internet or Nostr transport comes back.
- **`offline-protocol-telemetry-wire`**, a new crate holding the batch envelope
  and typed event payloads shared with the ingest, with the canonical wire
  fixture pinned byte for byte and a second fixture pinning the additive raw
  `reason` field.
- **A golden test against the frozen TypeScript client**: the same scenario
  replayed through both, compared batch by batch, with the two documented
  divergences (raw `reason`, the SDK version) normalized.
- **A debug tap.** `debug: true` logs every wire event on the platform log
  (logcat, the unified log) or stderr, which needed a place for the core's
  `tracing` output to go on a device; the uniffi crate now installs one.
- **An Apple privacy manifest** shipped with the pod, and
  [docs/privacy.md](docs/privacy.md) with the store disclosures.
- `ProtocolError.TelemetryConfigInvalid`, appended at position 24, naming the
  refused field.

### Changed

- **`TelemetryConfig` is reshaped** on every binding: `apiKey` and `appId` are
  required, `appVersion`, `debug`, `flushIntervalMs`, `maxBatchBytes`,
  `maxBufferedRecords` and `includeDeviceId` are new, `enablePollQueue` is
  gone, and the five earlier knobs keep their names and defaults.
- **`reason` goes up raw.** The earlier client classified the engine's failure
  reasons into a fixed family on the device; the pipe sends the engine's own
  token or literal and the classification happens on the ingest. Every reason
  the engine produces is locally chosen, per the threat model's producer rule.
- **`scrubIds` is moot for the pipe.** The engine still scrubs identifiers for
  the free event API; the pipe never reads one, so the option is accepted for
  compatibility and has no effect on what is uploaded.
- **Where the mesh-analytics package differed from the task, the task won:**
  a 15 min backoff cap rather than 5, a 413 dropped rather than split, a halt
  on 401 and 403 rather than a drop-and-continue, `Retry-After` honoured,
  `Idempotency-Key` and `User-Agent` sent, 15 s request and 10 s connect
  timeouts, and the battery deferral. Each is a named test.
- The `data` and `telemetry-pipe` cargo features are both on by default; a
  build that wants neither the CRDT engine nor the TLS stack opts out.
- `LICENSE-COMMERCIAL.md` states the telemetry term every commercial license
  carries: switching the pipe off or never supplying a key is permitted;
  repointing it at another service or reshaping what it uploads is not. The
  AGPL option is unchanged, and the event API is ordinary use under both.

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

## [0.25.0] — 2026-09-08

> **A device on Reticulum could never receive anything. It can now.** Both
> mobile managers attached to a daemon by opening a socket, sending `Identify`,
> announcing the carrier, and confirming every send the instant the write
> returned. They read two frame types and ignored the rest, so against a daemon
> answering [contract v1](docs/spec/gateway-contract.md) the session was never
> bound, and an unbound session may submit and be told a verdict but is never
> registered as a recipient. Nothing addressed to the device would ever have
> arrived, while the core settled every outbound frame as sent. The attach is
> now the handshake the contract specifies, sends settle on the gateway's
> verdict rather than on the write, and a `recipient_unreachable` reaches the
> classifier that parks a direct message and offers it to the mesh. **Reticulum
> is off by default** (`reticulum_enabled: false`), so this reaches only
> deployments that turned it on and pointed at a daemon; for those it is the
> difference between a transport that reported health and one that carries
> traffic.
>
> **The same change, read from the other end: a daemon speaking the older shape
> now gets a transport that connects and never becomes available.** The attach
> times out after ten seconds and the manager reconnects on its ladder, so the
> failure is a carrier that never arrives rather than an error. If you run your
> own daemon, [§19](docs/UPGRADING.md#19-the-reticulum-transports-inert-configuration-is-gone-v0250)
> names the frames it owes and the two new `SecurityWarningCode` values,
> `GATEWAY_ADDRESS_BINDING_MISMATCH` and `GATEWAY_ADDRESS_DECLARATION_REFUSED`,
> that explain a refused or mismatched attach.
>
> **An iOS phone that had stopped finding new peers was chasing peripherals
> whose owners no longer exist.** `willRestoreState` re-issued `connect(...)` on
> every peripheral iOS handed back, and the OS keeps servicing those requests
> across process relaunches for as long as the restore identifier is stable. A
> peripheral UUID belonging to a device that had been reset, reinstalled or
> thrown away was pursued until the app itself was reinstalled: no Bluetooth
> toggle, sign-out or force quit cleared it. Restoration is now partitioned, a
> pending connect survives only on evidence this device recently saw that
> peripheral advertise, and everything else is cancelled, which is the only
> thing that clears the OS-side queue. iOS only, no wire or FFI change, and
> nothing for an application to call.
>
> **The wire specification can be implemented from the text now, and vectors
> decide whether you got it right.** The binary v1 frame was specified as a
> field order and never said how an integer or a string becomes bytes, so a
> careful implementer would reasonably have written fixed-width big-endian
> fields and produced frames this protocol cannot decode. The chapter carries a
> normative primitive-encoding table, and seven vector files generated by a
> standard-library-only second implementation now pin the frame, address
> derivation, the control-plane and gateway signing payloads, the compact MLS
> envelope and the key package. A new [conformance chapter](docs/spec/conformance.md)
> states the two implementation profiles and what every implementation owes.
>
> **`ReticulumConfig` and everything around it is deleted, in the Rust crates
> only.** The type and its four fields, a second constructor, three methods, a
> builder and two constants, none of which anything read; `ReticulumTransport::new`
> is the one constructor left. No binding surface, configuration key or
> behaviour follows them out, and the React Native `transports.reticulum`
> section is untouched. TypeScript consumers who exhaustively `switch` on
> `PresenceSource` or `SecurityWarningCode` do get a compile error, because both
> unions widen.

### Added

- **The Reticulum transport speaks the gateway contract.** Both mobile managers
  attached to a daemon by opening a TCP socket, sending `Identify`, announcing
  the carrier, and confirming every send the instant the write returned. They
  read two frame types and ignored the rest, so a daemon answering
  [contract v1](docs/spec/gateway-contract.md) got nothing back: the session was
  never bound, which on the other side means it may submit and be told a verdict
  and is never registered as a recipient. Nothing addressed to the device would
  ever have arrived, and the core meanwhile settled every frame as sent.

  The attach is now the handshake the contract specifies: `Identify` with a
  version, a challenge signed under `offline-gateway-addr-v1`, `DeclareAddress`,
  the echo checked against this device's own address, capabilities, and only
  then the carrier announced. A refused declaration closes the connection rather
  than offering a transport that can only refuse, which is where this
  deliberately differs from the relay: an undeclared relay connection still
  works in account-name space, and a gateway has no such space. Two new
  `SecurityWarningCode` values, `GATEWAY_ADDRESS_BINDING_MISMATCH` and
  `GATEWAY_ADDRESS_DECLARATION_REFUSED`, report both answers.

  Sends settle on the gateway's verdict. `MessageSent` confirms, `DeliveryError`
  fails with the gateway's reason verbatim, and `recipient_unreachable` now
  reaches the classifier that parks a direct message and offers it to the mesh,
  which closes the Reticulum half of the mixed-neighbourhood residual
  `docs/mesh.md` has carried. Presence is watched from the SDK's own watchlist,
  and a gateway's answer un-parks over Reticulum rather than over the internet
  transport.

  `offline-gateway-addr-v1` is now a live signing domain with published
  [conformance vectors](docs/spec/conformance.md). The proof is built and signed
  in the core rather than in each bridge, so there is one implementation of the
  construction instead of one per platform.

  New FFI: `reticulum_address_declared`, `reticulum_address_declaration_refused`,
  `reticulum_gateway_capabilities`, `reticulum_peer_presence`,
  `reticulum_presence_watchlist` and `gateway_address_declaration`. New
  `PresenceSource` value `reticulum`, so an app filtering presence by carrier
  still can.

- **`RetryConfig.edge_driven_unreachable_dm`** (default `false`): an opt-in that,
  for deployments whose peers always interact or advertise presence on return
  (for example a machine-to-machine capability exchange), stops timed-probing a
  durably-unreachable direct message after a bounded number of probes and
  re-drives it only on a reachability edge, additionally bounding the core resend
  rate to gone peers. Default `false` preserves the documented perpetual-probe
  contract (a parked message "never goes fully quiet") so every existing native
  and third-party integration is unaffected. Exposed across the UDL, all
  generated bindings (Swift, Kotlin, Python), and the React Native TypeScript and
  native layers. See
  [docs/configuration.md](docs/configuration.md#reliability-configuration).

### Changed

- **The Python internet send loop is now activity-adaptive.** The relay bridge
  drained the core's outbox on a fixed 100 ms tick, so a reply to an inbound
  frame waited out most of a tick before leaving, and that wait dominated a
  warm round trip. An inbound frame now wakes the send loop immediately, a
  drain that moved frames re-drains at event-loop speed, and an empty drain
  backs off exponentially (2 ms doubling up to the unchanged 100 ms idle
  interval). A warm round trip drops from ~100 ms to single-digit
  milliseconds, with no standing busy-poll and no change to quiet-link cost.
  Python only; the Swift and Kotlin managers keep the fixed tick for now
  (see [docs/bridges/python.md](docs/bridges/python.md#p7-the-internet-send-loop-is-adaptive-here-and-fixed-elsewhere)).

### Removed

- **The Reticulum transport's inert configuration, in full.** `ReticulumConfig`
  and its four fields (`connection_timeout`, `auto_reconnect`, `reconnect_delay`,
  `max_reconnect_attempts`), `ReticulumTransport::with_config`, `config()`,
  `should_reconnect`, `increment_reconnect_attempts`, `ReticulumTransportBuilder`
  and the constants `RETICULUM_CONNECTION_TIMEOUT_SECS` and
  `RETICULUM_MAX_PAYLOAD_SIZE` are deleted. Nothing read any of them:
  reconnection is owned by the native managers and configured from the app's
  transport config, `reconnect_delay` had no reader at all, and the payload
  constant was never enforced anywhere. `ReticulumTransport::new` is now the
  only constructor. **Rust-library consumers only**: the FFI never threaded
  any of it, so no binding, no configuration key and no behaviour changes.
  See [docs/UPGRADING.md](docs/UPGRADING.md#19-the-reticulum-transports-inert-configuration-is-gone-v0250).

### Fixed

- **Re-discovering an established iOS BLE link no longer repeats the work that
  set it up.** `BleManager`'s characteristic-discovery callback is not a
  first-contact callback: the connection monitor re-runs characteristic
  discovery every five seconds on any peripheral missing from the connection
  registry, and CoreBluetooth replays the callback from its cache when it does,
  so a live connection reaches it indefinitely. Every arrival re-subscribed to
  the message characteristic and re-read both halves of the peer handshake.

  The subscribe now runs only when the characteristic reports that it is not
  already notifying. A redundant one re-emitted the bridge's own subscribe
  diagnostic, so a link that subscribed once read in the logs as one
  re-subscribing every sweep, and where the write reached the peer it re-ran
  that peer's inbound admission decision, which can evict a different peer.
  The handshake reads now stop once the peer is announced, which also drops a
  signature verification and an address derivation that ran on the main thread
  on every sweep and whose result was discarded. A disconnect still resets both
  gates, so a reconnect re-subscribes and re-proves the peer from scratch.

- **iOS state restoration no longer chases peripherals whose owners are gone.**
  `BleManager.centralManager(_:willRestoreState:)` re-issued `connect(...)` on
  every peripheral iOS handed back. The OS keeps servicing those connect
  requests across process relaunches for as long as the restore identifier is
  stable, and the only clear the app itself controls is
  `cancelPeripheralConnection` on the instance the system hands back, so a
  peripheral UUID whose owner had stopped existing was chased until the app was
  reinstalled. No Bluetooth toggle, sign-out or force quit cleared it. The
  visible symptom was a phone that would not discover a legitimate new peer,
  and a device burning battery on connect requests that could never complete.

  The restored list is now partitioned. A peripheral handed back in
  `.connected` state is restored unconditionally: the link is alive at that
  instant, and it is usually the reason iOS relaunched the app. A pending
  connect is restored only if this device saw that peripheral within 60 seconds
  of the last thing it saw before it was terminated, tracked in a small
  last-seen map that survives the relaunch. Everything else is cancelled, which
  is what clears the OS-side queue. Sightings are recorded from advertisements,
  from completed connections, and from traffic arriving on a live link. An
  advertisement counts as seen before the adaptive filters that shed scanning
  work in dense environments, so a peer one of those filters skips is not
  mistaken for a peer that stopped advertising.

  That 60 seconds is measured against the newest advertisement the scan
  received, not against the relaunch clock and not against the newest entry in
  the map. Nothing records a sighting while the app is dead and iOS relaunches
  it hours later, so measuring against the relaunch clock would cancel every
  pending connect at every restoration. Measuring against the newest entry has
  the same effect by a slower route, because traffic on a live link keeps
  refreshing an entry while the app is backgrounded and the scan is stopped: a
  single connected peer would push the window past every absent peer's last
  sighting and cancel all of them. An advertisement is the only observation
  that proves the app was in a position to see anything at all. A pending
  connect is the only way iOS wakes the app when a known peer reappears, since
  a background scan with no service filter is ignored by the OS, so either
  mistake would have traded a visible bug for an invisible one.

  Both halves of the decision are issued once the central reports `.poweredOn`,
  not where the restored list arrives. `willRestoreState` is delivered before
  `centralManagerDidUpdateState`, and CoreBluetooth discards a command issued
  before then, so cancelling on arrival would leave the OS-side queue intact
  while the diagnostics reported the peripherals as dropped. The failed-connect
  retry also now skips a peripheral that has been dropped from the discovered
  map, so a cancelled connect request is not immediately re-armed.

  A restored peripheral that is still only connecting is no longer booked as a
  live connection, so it neither spends a connection slot nor suppresses the
  retry that is supposed to chase it.

  The last-seen map is device-scoped, holds only OS-assigned peripheral UUIDs
  and timestamps, and is not account state, so `wipePersistedState()` does not
  touch it (see [docs/bridges/README.md](docs/bridges/README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified)).
  iOS only. No wire, FFI or configuration change.

- **Internet-only providers no longer flap a relay's rate limiter into a
  disconnect loop under a backlog of unreachable direct messages.** Three
  compounding defects drove unbounded resends to gone peers. (1) A resend that
  had to register a fresh ACK (for example after an ACK-timeout re-queue)
  restarted the backoff ladder at `retry_count` 0, pinning `delay_for_retry` at
  its 1s floor forever for a never-ACKing recipient;
  `AckManager::set_retry_count` now carries the retry-queue entry's accumulated
  count onto the fresh ACK so backoff continues instead of resetting. (2) An
  unconfirmed peer that vanished was re-probed every 5s indefinitely; the
  confirmation probe now escalates on the same 15s to 600s ladder as the welcome
  lifecycle and resets on a reachability edge. (3) The Python relay bridge
  dropped the relay's recipient-keyed `DeliveryError` verdict; it now correlates
  in-flight sends per recipient (a port of the iOS/Android
  `RecipientInFlightTracker`), fails the affected message ids fast, and feeds
  `internet_peer_presence(online=false)`. Default behavior is otherwise
  unchanged; the always-on fixes only reduce redundant relay traffic.

### Documentation

- **The wire specification now says how a primitive becomes bytes, and seven
  vector files hold it to that.** `docs/spec/wire-format.md` specified the
  binary v1 frame as a field order (`id 16 raw bytes`, `sender string`,
  `timestamp i64`) and never stated the encoding of an integer or a string. The
  implementation uses varints: unsigned values are LEB128, signed values are
  zigzagged first, strings and byte sequences carry a varint length prefix, and
  the two 16-byte identifier fields carry none. An implementer reading the
  chapter would reasonably have written fixed-width big-endian fields and
  produced frames this protocol cannot decode, misaligned from the first varint
  onward. The chapter now carries a normative primitive-encoding table, states
  the identifier and Lamport-clock bounds as numbers, and says that metadata
  keys sort byte-wise on their UTF-8, which is the same order as by code point
  but not the order a Java, JavaScript or C# string comparison produces above
  U+FFFF.
  Conformance vectors now cover the binary frame, address derivation and
  parsing, the session slot, the two control-plane canonical signing payloads,
  the gateway address-declaration proof, the compact MLS envelope, and the key
  package payload's parse behaviour. They are generated by
  `tools/spec-vectors/generate.py`, a standard-library-only second
  implementation written from the chapters that never reads the crates it pins,
  and CI regenerates and diffs them so an expectation cannot be edited to turn a
  red test green. A new `docs/spec/conformance.md` states the two
  implementation profiles, what every implementation owes, what the vectors
  deliberately do not cover, and how a second implementation self-tests.
  No runtime behaviour changed.

- **The signing-domain registry omitted a live domain.** The table in
  `docs/spec/username-discovery.md` listed four live domains and still does not
  appear anywhere else, but `offline-ctrl-v2` has been live since control-frame
  freshness shipped, making five. `offline-gateway-addr-v1` joins them in this
  release, so the table now carries six. Non-prefixing is a property of the
  whole set, so a registry that cannot see every member cannot be used to choose
  a new domain safely, which is what that table exists for. The row is now present and
  a test fails if a domain the control-signing vectors pin is missing from it.

- **The service-message prefix was reserved but never published.**
  `docs/spec/control-messages.md` referred to "a generic service message prefix"
  without naming it. It is `__SVC_`, and it bounds a namespace rather than
  naming a frame, which the chapter now states: a conformance check that treats
  the registry as a set of frame tags fails on that entry alone. The reserved
  prefix registry is now checked against the chapter in both directions, so a
  prefix reserved here and unpublished there, or published there with no
  constant behind it, fails the build.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
