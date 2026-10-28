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
  subscription survives a restart, and a frame from before one is dropped. Who may subscribe is the service's policy: ask the application
  (the default), accept every peer with a session, or rules by peer and group.
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

- **`sendReadReceipt` works again from React Native on Android.** Since the
  shared engine bridge (0.31.0), every array argument crossed the Android
  adapter as integers, so the message ids, which are strings, threw on every
  call. Arrays now keep each element's type, through the same conversion an
  array inside a map already used, and byte arrays still reach the engine
  unchanged. Empty ids are dropped as they were before, now on iOS too, so
  both platforms and Flutter apply one rule. A guard holds every array
  parameter of the adapter to that conversion, and JVM tests pin what it
  returns.

## [0.31.0] — 2026-10-27

> **The engine runs in a browser (experimental).** `offline-protocol-web` and
> `bindings/web` run the full engine, MLS included, in a Web Worker. A browser
> node is durable over origin-private files, links to other devices over
> WebRTC, Nostr and Web Bluetooth, and finds the browsers on its own network
> through a nearby room.
>
> **Two devices that share only a Nostr relay form a session.** A send to a
> peer with no session now asks the relays for the key packages it published,
> on iOS, Android and Flutter as in the browser, so an application that turned
> Nostr on sends new queries to its relays. A pair split into two sessions, or
> left without one by a lost reset, now heals, and an old group message no
> longer fills a peer's send window.
>
> **Also new:** a Ruby binding, published to RubyGems; live Bluetooth LE signal
> strength as a `peer_rssi` event; and an iPhone that names a central it
> cannot dial back.
>
> **Rust code that names every case or every field breaks in four places.**
> `Event` gains `PeerRssi`, `SendMessageOptions` and `GroupSendOptions` gain
> `ephemeral`, and `offline-protocol-sealed`'s `KeyPackagePayload` gains
> `no_session`. An exhaustive TypeScript `switch` over the event union needs a
> `peer_rssi` case. Nothing is removed from any binding; what behaves
> differently is in
> [§29](docs/UPGRADING.md#29-behaviour-that-changes-without-a-compile-error-v0310).

### Security

- **Nostr signing and NIP-44 key agreement no longer run on a timing-leaky
  big-integer library.** RUSTSEC-2026-0387 reports secret-dependent branches
  in `crypto-bigint` below 0.7.6, which `k256` 0.13 builds its scalar
  arithmetic on, so the Nostr transport's Schnorr signatures and its NIP-44
  conversation keys did that arithmetic on the identity's secret key. The
  workspace now takes `k256` 0.14, on `crypto-bigint` 0.7.10 (upstream yanked
  0.7.6 to 0.7.8, the first fixed releases, for an unrelated API break). Keys,
  routing tags, discovery tags and record seal keys are unchanged: the golden
  values and the official NIP-44 vectors pass as before. `crypto-bigint` 0.5.5
  stays in the build only under the MLS and HPKE libraries' P-256, P-384 and
  secp256k1 suites, which hold none of the SDK's keys (every key is X25519 or
  Ed25519), and is waived in `deny.toml` until those libraries move to
  `elliptic-curve` 0.14.

### Added

- **The engine runs in a browser (experimental).** `offline-protocol-web`
  compiles the full engine, MLS included, for `wasm32-unknown-unknown`, and
  `bindings/web` runs it in a dedicated Web Worker. Tabs of one origin find
  each other over a `BroadcastChannel`, prove their identities with the
  peer-stream preamble, and exchange end-to-end encrypted direct and group
  messages with no server; see [docs/bridges/web.md](docs/bridges/web.md) and
  [examples/web-two-tabs](examples/web-two-tabs).
- **A browser node is durable.** Its identity, groups and delivery state live
  in sealed append logs over origin-private (OPFS) files, so a reload or a
  crash comes back as the same device and still a member of its groups. Every
  write is flushed before it is acknowledged, every record is sealed and
  bound to its position, and the store key is itself sealed under a
  non-extractable WebCrypto key. One node runs per account per origin; a
  second is refused. `storage: "memory"` keeps the old behaviour. See
  [W8](docs/bridges/web.md#w8-storage-is-a-sealed-log-one-node-per-account),
  and [R26](docs/security/threat-model.md#r26-a-browser-nodes-store-key-is-reachable-from-its-origin)
  for what the sealing does not protect against.
- **Browsers connect over WebRTC with no server.** `WebRtcCarrier` links two
  devices on one network with one data channel, set up from two codes the
  people exchange by QR code or paste; no ICE server is configured. See
  [W9](docs/bridges/web.md#w9-webrtc-lives-on-the-page-and-is-set-up-offline),
  [R27](docs/security/threat-model.md#r27-a-webrtc-connection-code-is-unauthenticated-and-names-local-addresses)
  and [examples/web-lobby](examples/web-lobby), a two-player Pong game.
- **Browsers can meet by room code through Nostr relays.** `NostrSignaller`
  carries the two WebRTC codes through relays the application names, sealed
  under a key derived from a room code and signed by single-use keys. See
  [W11](docs/bridges/web.md#w11-signalling-can-ride-a-nostr-room) and
  [R28](docs/security/threat-model.md#r28-a-signalling-room-code-admits-whoever-holds-it).
- **`WebRtcCarrier` takes the application's STUN and TURN servers**
  (`iceServers`), for links between networks. The default is still none.
- **`WebRtcCarrier` reports each connection's state.** A `connection` event
  says when a connection is up, when its checks stop being answered
  ("disconnected", which may recover), and when it closed, naming the object
  `offer()` or `answer()` resolved to. A page can warn about a link seconds
  before the browser gives up on it and the engine reports the peer lost.
  A call that rejects reports nothing, and a connection closed while its
  code was still gathering makes `offer()` or `answer()` reject rather than
  resolve to a code nobody can use.
- **The browser reaches devices it has no link to, over Nostr.**
  `OfflineNode.start({ nostr: { relays } })` adds the Nostr transport every
  binding shares, driven from the worker over the relays the application
  names; the SDK names none and refuses an empty list. A message to a bare
  address fetches the key package the device published and forms the
  session, so two nodes that share only a relay can talk. It is the
  transport and wire format the mobile bindings run; a browser to phone pair
  has not been run on devices yet.
  `resolveUsername` reads the username directory (`usernameDiscovery`, off
  by default, publishes this profile and resolves others'), and
  `establishSecureSession`, `createInvite` and `parseInvite` match the
  native calls ([W12](docs/bridges/web.md#w12-the-nostr-transport-is-driven-from-the-worker-over-the-applications-relays)).
- **Browsers on one network find each other.** `NearbyRoom` meets the
  browsers behind one public address in a Nostr room derived from that
  address and the application id, lists them by the label each application
  chose, and links over WebRTC to the one a person picks; the link runs on
  the local network. The room key is public by construction, so every
  connection code is sealed to one device's key, every presence is signed
  by the key it announces, and a host answers only what its `admit`
  callback accepts, asking once per session. Relays and STUN servers are the
  application's ([W13](docs/bridges/web.md#w13-a-nearby-room-is-derived-from-the-networks-address),
  [ADR 0034](docs/adr/0034-browsers-find-each-other-through-their-network-address.md)).
- **A browser links to a phone or a board over Bluetooth LE.** In
  Chromium, `BluetoothCarrier` makes the page a GATT central: its person
  picks a device running the SDK in the browser's chooser, the engine
  verifies the identity the device serves, the page writes its hello, and
  the link carries sessions and messages through the same `BleTransport`
  and fragment codec as every binding. A browser is never discoverable over
  Bluetooth, and Firefox and Safari have no Web Bluetooth
  ([W14](docs/bridges/web.md#w14-bluetooth-is-a-central-the-person-points-at-a-device)).
  Not run yet from a real browser to a real device.
- **The browser can remove a group member.** `OfflineNode.removeFromGroup`
  removes a member from a group this device administers: the others move to
  a new epoch whose keys the removed member never holds, and the removed
  member gets `group_member_removed`. `OfflineNode.groupInfo` returns a
  group's members and epoch, or null for a group the device does not hold.
  Until now a browser could create and administer a group but never shrink
  it ([web bridge](docs/bridges/web.md)).
- **The browser engine can start from bytes.** `OfflineNode.start` takes
  `wasm` (the module's bytes or a compiled module) for a host that cannot
  serve the module beside `worker.js`, such as an editor plugin whose worker
  starts from a Blob URL.
- **Ephemeral sends for state the next send replaces.**
  `SendMessageOptions::ephemeral` and `GroupSendOptions::ephemeral` (and
  `{ ephemeral: true }` on the browser's `sendMessage` and
  `sendGroupMessage`) send once over a live direct link, ask for no
  acknowledgement and keep nothing to retry, so a game's positions never
  arrive late. With no link, or no confirmed session, the call fails at once.
  A group member who keeps missing them gets a reliable message every 256
  sends, so their ratchet never falls out of reach, and is handed nothing
  newer until that message is acknowledged. On the browser's store an
  ephemeral group message costs the sender two flushes against four. Not yet
  on the native bindings. See
  [ADR 0032](docs/adr/0032-ephemeral-sends-go-once-over-a-live-link.md) and
  [W10](docs/bridges/web.md#w10-an-ephemeral-send-goes-once-over-a-live-link).
- **A Ruby binding.** `bindings/ruby` is one generated file,
  `lib/offline_protocol.rb`, written by UniFFI's own Ruby generator from the
  interface definition with the other bindings, so it needs no generator
  beyond the `uniffi-bindgen` the rest already use. It calls the native
  library through the `ffi` gem, with no glue of its own and not through the C
  API. `OfflineProtocol::OfflineProtocol`, `MeshServices`, `DataStore`, every
  record and enum, the objects a host implements (events, transports, storage
  providers) and an exception per engine refusal. Unlike the generated Java,
  the records keep the definition's field defaults. CI regenerates and diffs
  the Ruby with the rest, and runs a minitest consumer check against the
  library on Linux, macOS and Windows. A config names the library, because the
  generator's default is a name no build produces. An engine that Ruby's
  garbage collector frees while it holds a callback leaks the callback and
  warns, which is a defect in the generator and is documented as such. The
  release builds it as the `offline_protocol` gem, which carries no native
  code, and publishes it to RubyGems with trusted publishing under the
  `RUBYGEMS_PUBLISH` switch. The contract is `docs/bridges/ruby.md`.
- **Live Bluetooth LE signal strength.** A new `peer_rssi` event
  (`{ peer_id, transport, rssi }`) reports a linked peer's RSSI from every
  scan, at most four a second per peer, and only while the app listens for
  it; see [peer_rssi](bindings/react-native/README.md#peer_rssi). The engine
  attributes each reading to the peer itself, so an app no longer joins
  scans to peers. With `transports.ble.advertiseNodeHash: true` (off by
  default), Android advertises an 8-byte digest of its address as service
  data under the mesh UUID in its scan response, in place of the bare UUID,
  so an iPhone can name an Android peer (Android re-advertises from a new
  address on every link change, so without it an iPhone rarely can). The
  digest never changes, so any scanner in range can follow the phone across
  Bluetooth address rotation, and anyone who knows its address can identify
  it ([R29](docs/security/threat-model.md#r29-the-bluetooth-le-scan-response-names-the-device-and-rssi-is-unauthenticated)).
  While the app listens, readings also keep each link's quality current,
  smoothed so that one reading's noise does not reorder neighbours; the mesh
  relay ranks neighbours by it, at most four readings a second per peer. An
  app that never listens keeps the discovery-time reading, as before; once
  the last listener goes, each link keeps its last smoothed reading.
- **Shared React Native engine bridge.** Engine operations live in
  `ProtocolEngineHost` and `ProtocolEngineHostBridge`; `OfflineProtocolModule`
  delegates through `ProtocolEngineHostBridgeReactNative` with promise fallback
  codes pinned in `tools/rn_engine_bridge_error_manifest.json`. JavaScript sees
  no change: the same file records the 19 methods that reject with typed
  protocol codes and the one (`getDorsConfig`) whose JSON is resolved as an
  object, and every other string reaches JavaScript unchanged. Generators and
  `scripts/check-binding-generators.sh` keep Kotlin and Swift adapters in sync
  ([C9b](docs/bridges/README.md#c9b-shared-engine-bridge-generators-and-rn-promise-codes)).
- **Native packages after the shared engine bridge.** SwiftPM and the Android
  library omit React-only bridge files (`ProtocolEngineHostBridgeReactNative`,
  bridge argument/result maps, and the RN module). The Flutter iOS pod and
  Android Gradle build use the same leave-out lists and pick staged UniFFI
  artifacts only when a real library is present ([ADR 0025](docs/adr/0025-native-packages-are-assembled-in-place.md)).
- **Flutter engine method channel.** The plugin routes Dart calls through a
  generated `FlutterEngineChannel` on Android and iOS into the shared engine
  host, with `method_channel_methods.txt` as the single manifest. Reticulum
  and Nostr configs are no longer refused: the shared host serves both.
  `MeshSdk` exposes the same surface to Dart (a few typed helpers, and a
  map-argument wrapper per remaining method), and
  `scripts/check-binding-generators.sh` gates the Flutter generators
  ([docs/bridges/flutter.md](docs/bridges/flutter.md)).
- **Flutter binding parity and iOS CI.** Rust guards pin React Native parity
  for mesh foreground keep-alive, sticky one-shot events, and inbound event
  buffering on Android and iOS. A new `Flutter Binding (iOS bridge)` job runs
  `bindings/flutter/scripts/ci-typecheck-flutter-ios-bridge.sh`, which
  typechecks the plugin's Swift with the shared bridge sources the pod
  compiles; until now nothing in CI compiled the iOS plugin. On Android,
  `start()` now runs the mesh foreground service React Native runs, so the
  plugin's manifest adds `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_CONNECTED_DEVICE`, `CHANGE_WIFI_STATE` and a
  `connectedDevice` service, and the device shows the service's notification
  while the mesh runs. `stop()`, and destroying or detaching the engine, stop
  the service after the transports rather than before them.

### Changed

- **No shipped file names a Nostr relay.** The docs, doc comments and the
  Nostr example listed public relays; they now show a placeholder, and the
  example starts with an empty list for the developer to fill.
  `no_shipped_file_names_a_relay` keeps it so: a relay the SDK names becomes
  the relay every application uses.
- **The Nostr driver guide no longer completes a resolution on the first
  EOSE.** `docs/nostr.md` said so while the bridges and the username chapter
  wait for every relay; the guide now says what the code does.
- **The workspace reads the clock through `offline_protocol_core::time`.** It
  is `std::time` on every target but the browser, where `std::time` compiles
  and panics on first use; there it is `web-time`. Nothing changes on any
  existing platform: the types are the standard library's own.
  `clocks_are_read_through_core_time_only` refuses a direct import.
- **`account_storage_namespace` no longer needs the `file-store` feature.**
  The browser's store derives its folder with it, and a browser build has no
  POSIX files. It is still re-exported from `offline_protocol::file_store`.
- **A decrypt failure names its cause.** The `reason` of
  `message_decryption_failed`, for a message or a media chunk, now carries
  the OpenMLS failure as a fixed name
  beside the kind, for example `(InvalidCiphertext, aead)` or
  `(InvalidCiphertext, too_distant_in_the_past)`. A pair holding two
  different sessions for one slot fails as `aead` on every frame, where a
  frame too far behind a healthy session does not, and the two were
  indistinguishable before. Only names from a closed list are written, so
  nothing the error renders reaches telemetry. `MlsError::decryption_detail`
  reads it.
- **Telemetry counts only acknowledged sends as sent.** An ephemeral message
  is never delivered or failed, so counting it would lower the delivery ratio
  by every game update.
- **The browser tests run in Chromium, Firefox and WebKit.** Playwright drives
  every page in `bindings/web/test` in all three, and both storage
  conformance suites now run against the log store over real OPFS files in a
  worker of Chrome and Firefox, with a reopen through compaction. On a target
  with no threads, the MLS suite's concurrent-writers check runs its writers
  in turns instead of failing to start them. A two-device checklist for the
  lobby is in [docs/bridges/web-devices.md](docs/bridges/web-devices.md).

### Fixed

- **A device known only by its address is reached over Nostr from a phone
  too.** Cold first contact never started outside the browser: a send to
  a peer with no session only queued, and nothing asked the relays for the
  key packages that peer published, so two phones sharing only a relay never
  formed a session. The engine now asks when a send or
  `establish_secure_session` finds no key package for the peer, and the
  tick asks again every 5 minutes while a message waits. A peer on a live
  Bluetooth or Wi-Fi link, a blocked peer, and every peer with cold contact
  off are never looked up. The browser's own lookup is gone; the engine's
  replaces it.
- **An iPhone names a central that cannot be dialled back.** The iOS
  peripheral now serves the Hello characteristic, as Android and the Python
  service do: a central writes its identity assertion once, and the iPhone
  binds the link to the address it proves instead of queueing every fragment
  behind a dial-back. A central with no GATT server of its own (a browser
  over Web Bluetooth) could never be named before. Writes are refused before
  verification when long, offset or out of bounds, and the per-peer MTU
  reaches the core before the peer does. Only a central the iPhone kept is
  named, its link counts toward the mesh's connection budget at once, and
  the iPhone waits two seconds for a hello before it dials a new central
  back. A peer whose last link was a central that left is now reported lost
  after 15 seconds, as on Android; before, no callback reported it, and the
  core kept sending to a link that was gone
  (`react_native_ios_peripheral_serves_hello`).
- **An old group message no longer blocks every later message to that
  device.** A group frame further behind the receiver's ratchet than the
  out-of-order window can never decrypt, and the sender resends the same
  ciphertext, because it never re-seals a group frame. The receiver buffered
  it as "may decrypt later" and withheld its ACK, so the sender kept
  resending it for the outbox lifetime of seven days, and a restart or
  relink reset its retry budget. Each such frame held one of the sixteen
  slots in that peer's send window. Between a Mac and a phone that had been
  through a day of app kills and restarts, sixteen of them filled the window,
  and for three and a half hours nothing else got through, a join request
  included. The receiver now acknowledges such a frame from a group member
  and drops it. From anyone else it stays unanswered, as before.
- **A browser device can link again after the other one restarts.** The
  WebRTC carrier closed a connection only on `failed` or `closed`, and a
  browser need never get there: Android's WebView was seen to sit in
  `disconnected` for good once the other device went away. The engine takes
  one link per address, so that dead link refused every new link from the
  same device until a page reloaded. A connection that stays `disconnected`
  for 15 seconds is now closed and reported lost; `new WebRtcCarrier(node,
  { disconnectedMs })` sets the deadline, which is longer than Chrome's own
  so Chrome behaves as before
  ([web bridge, W9](docs/bridges/web.md#w9-webrtc-lives-on-the-page-and-is-set-up-offline)).
- **A room hosted through Nostr keeps answering after the device sleeps or
  changes networks.** `NostrSignaller.host` opened its relay sockets once
  and never again, so once a phone slept or lost Wi-Fi the room answered
  nobody, while `host()` had long since resolved. A hosted room now redials
  a relay that drops, with backoff from one second to thirty, asks each live
  relay to confirm its subscription every `keepAliveMs` (default sixty
  seconds) and redials one that does not, or that accepts a redial and never
  confirms it, and does both at once when the page comes back online or into
  view ([web bridge, W11](docs/bridges/web.md#w11-signalling-can-ride-a-nostr-room)).
- **A pair no longer splits into two sessions that never decrypt each
  other.** When both sides created a session at once and the lesser
  address's Welcome was lost, the greater side kept a group the other never
  joined, and the other side's plaintext confirmation probe confirmed it.
  Every message after that failed as `INVALID_CIPHERTEXT`, and nothing healed
  it, because the two groups sit at the same epoch and the re-key fires only
  on an epoch mismatch. A probe and its acknowledgement now carry an MLS
  exporter tag that only a member of the same group computes. A match
  confirms, including the session owner, which before only a decrypted
  message could. A mismatch is a split, and the greater side heals it by
  giving up its own group, so the other side resends its Welcome and keeps
  its group. It does so only when the mismatch answers a challenge it issued
  under its current session, so a late or replayed probe can never tear a
  healthy session down. An older peer sends no tag and keeps the old
  rules.
  A split between two sides that had both confirmed is found too: an ordinary
  decrypt failure on a 1:1 session now sends a probe, at most once per peer
  every 30 seconds, and the tags decide. The failure itself still tears
  nothing down. A message decrypted in a multi-party group the pair shares
  no longer confirms their 1:1 session, which let a split confirm. See
  [ADR 0033](docs/adr/0033-a-session-is-confirmed-only-by-proof-of-the-same-group.md).
- **A lost re-key no longer leaves one side without a session for good.**
  When the reset frame of a re-key or an unblock never arrived, the side that
  sent it held no session, the other kept its own, and the first side's
  repeated key packages only refreshed the other's copy. Messages then waited
  on both sides until they expired. A key package sent while the sender
  holds no session now says so (`no_session`). It is omitted when false, so
  a package that does not set it is byte for byte what it was, and an older
  receiver ignores it where it is set, which is every push from a side
  holding no session, first contact included. The holder sends its Welcome
  again when that Welcome was built on the very package offered, since the
  sender never processed it, and otherwise drops its session and rebuilds,
  under the same freshness and spent-reset rules as a reset. A flag older
  than the frame that built the session is refused, and the same rule now
  refuses a late `session_reset`. A Welcome given up in
  the both-create tiebreak is discarded, so it is never sent again.
- **A durable browser node starts in Firefox.** `OfflineNode.start` awaited
  `navigator.storage.persist()`, which Firefox answers by asking the user, so
  the start waited for the prompt (and never finished in a headless
  browser). The request is now made without waiting for its answer.
- **A browser log store whose compaction fails no longer retries it on every
  write.** The next attempt waits for another mebibyte of log.
- **A corrupted length prefix in the browser log store is refused, not cut.**
  It read as a torn tail, so acknowledged records after it were truncated.
  The log is now refused untouched whenever an intact record follows the
  damage anywhere. Replay also reads the log in one call instead of two per
  record.
- **A slow WebRTC peer no longer grows the page's memory without bound.** A
  connection with more than 8 MiB waiting behind the channel is closed.
- **A browser node's calls after `stop()` reject** instead of waiting for
  ever, and the tab carrier no longer throws on a frame queued as it closed.
- **One unreachable Nostr relay no longer stops a signalling room from
  opening on the others.** A refused socket was counted as two failed
  relays.
- **A signalling host no longer forgets a joiner whose offer is still
  fresh.** It held each session for two minutes from the first offer, while
  a republished offer, or one from a joiner whose clock runs ahead, stays
  acceptable for longer, so a relay's late repeat opened a second
  connection. The worker now reports when each offer stops being fresh, and
  the host holds the session until the newest one does.
- **The Pong lobby shows a lost link on both sides.** A player whose phone
  went to the background or locked its screen kept reading "Joined the game"
  over a frozen court, because the page only watched for a lost peer while
  hosting. Both sides now say the other device disconnected and how to carry
  on, and warn while the connection is unstable. The game state and the
  paddle are ephemeral sends, so a long absence no longer piles up states
  that all arrive on the next link; a returning device is let back in without
  a new invite; and a host that reloads resumes the same game. The host keeps
  inviting a device for as long as it holds a link, where a fixed number of
  tries could give up before the device's key package arrived and leave it
  never invited with nothing on screen. An invite that keeps failing, or game
  updates that stop reaching anyone, are shown with the engine's reason, and
  a resumed game the node no longer holds points at **End this game**.
- **A signalling room that fails to open closes its relay sockets.** A relay
  that took the connection and never confirmed the subscription left its
  socket open after `host()` rejected.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
