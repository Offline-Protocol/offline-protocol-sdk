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
- **Browsers can meet by room code through Nostr relays.** `NostrSignaller`
  carries the two WebRTC codes through relays the application names, sealed
  under a key derived from a room code and signed by single-use keys. See
  [W11](docs/bridges/web.md#w11-signalling-can-ride-a-nostr-room) and
  [R28](docs/security/threat-model.md#r28-a-signalling-room-code-admits-whoever-holds-it).
- **`WebRtcCarrier` takes the application's STUN and TURN servers**
  (`iceServers`), for links between networks. The default is still none.

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

### Changed

- **The workspace reads the clock through `offline_protocol_core::time`.** It
  is `std::time` on every target but the browser, where `std::time` compiles
  and panics on first use; there it is `web-time`. Nothing changes on any
  existing platform: the types are the standard library's own.
  `clocks_are_read_through_core_time_only` refuses a direct import.
- **`account_storage_namespace` no longer needs the `file-store` feature.**
  The browser's store derives its folder with it, and a browser build has no
  POSIX files. It is still re-exported from `offline_protocol::file_store`.
- **Telemetry counts only acknowledged sends as sent.** An ephemeral message
  is never delivered or failed, so counting it would lower the delivery ratio
  by every game update.

### Fixed

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
- **A signalling room that fails to open closes its relay sockets.** A relay
  that took the connection and never confirmed the subscription left its
  socket open after `host()` rejected.

## [0.30.0] — 2026-10-21

> **Update if you enable telemetry.** The telemetry uploader in 0.26.0 to
> 0.29.0 trusted every root in the bundled Mozilla set, one of them an
> eMudhra root withdrawn after the October 2026 key compromise, so a holder
> of those keys on the network path could read the telemetry API key and
> the batches. It now trusts only the sixteen roots the ingest's issuers use.
>
> **Three new ways in: C, Java and Flutter.** A C header over the native
> library every release already ships, a Java binding generated from the
> interface definition (JDK 22+, on Maven Central), and a preview Flutter
> plugin for iOS and Android. React Native gains `sendFileFromPath` and
> `signDataBytes` for large payloads.
>
> **Bluetooth LE holds a saturated link.** A 10,000-message two-phone volume
> test found a link offered more than it carries collapsing into mass
> retries, a full acknowledgement table stopping the engine, and a receiver
> falling permanently behind its sender's ratchet. Sends to one nearby peer
> are now windowed at sixteen in flight, acknowledgements jump the queue and
> coalesce, and a full outbox refuses the newest message instead of failing
> the oldest.
>
> **Smaller libraries, newer toolchains.** Link-time optimisation now
> reaches every shipped library, and iOS and Android build for size. UniFFI
> moves to 0.32: building the native library from source needs Rust 1.91,
> and the iOS bridge needs Swift 5.10 (Xcode 15.3).
>
> **Dashboards read lower.** Telemetry counts only the application's own
> messages as delivered or failed, so the delivered count stops including
> media chunks and internal frames.
>
> [§28](docs/UPGRADING.md#28-behaviour-that-changes-without-a-compile-error-v0300)
> lists what changes without a compile error.

### Security

- **The telemetry uploader trusts only the ingest's issuers.** Until now it
  trusted the whole Mozilla root set bundled through `webpki-roots` 1.0.9,
  which still carried emSign ECC Root CA - G3, one of the roots withdrawn
  after eight eMudhra intermediates were found with weak keys in October 2026.
  Anyone holding one of those keys on the network path could have presented
  a certificate for the ingest and read the telemetry API key and the
  batches. The uploader now trusts sixteen roots, the server roots of the
  four authorities the ingest's edge issues from (Let's Encrypt, Google Trust
  Services, SSL.com, and Sectigo for Cloudflare's backup certificate), pinned
  by SHA-256 in a test, so a compromise of any other authority no longer
  reaches the SDK. They include Let's Encrypt's Generation Y roots, ISRG Root
  YE and YR, which issue the ingest's certificate today, so the chain still
  validates if the edge stops sending the cross-signs to the older ISRG roots.
  `webpki-roots` leaves the dependency graph, `ureq` drops to
  `rustls-no-provider`, and the uploader names the ring crypto provider
  instead of taking a process default. `rustls-webpki` moves to 0.103.16,
  which rejects any path through the compromised eMudhra intermediates' keys
  whatever root it ends at, closing the cross-signed path through Sectigo's
  AAA Certificate Services as well. Nothing
  changes for an application: the endpoint, the proxy handling and the
  device-store exclusion are as before. Applications on 0.26.0 to 0.29.0 that
  enable telemetry should update. See
  [ADR 0031](docs/adr/0031-telemetry-trusts-only-the-ingests-issuers.md).


### Added

- **A Flutter plugin (preview).** `bindings/flutter/offline_protocol` hosts
  the engine on iOS and Android from Dart: create, MLS with the platform
  secure stores, start, stop, destroy, the local address, `sendMessage`, the
  Internet, BLE and Wi-Fi Direct transports, and core events with the same
  sequence numbers and gap replay as React Native. Its Android and iOS hosts
  compile the React Native bridge sources where they are, less a named list
  of the ones that need React, so a transport fix reaches both. Nostr and
  Reticulum are not hosted yet, and a config that enables either is refused
  at create. Until it is on pub.dev it is a path dependency. CI runs the Dart
  tests and assembles the Android plugin against a size baseline; nothing in
  CI compiles the iOS plugin yet. The contract is `docs/bridges/flutter.md`.
- **A Java binding.** `bindings/java` is a typed Java API generated from the
  interface definition by `uniffi-bindgen-java`, the fourth member of the
  artifact set that `scripts/generate-bindings.sh` writes with Swift, Kotlin
  and Python. It reaches the native library through the Java foreign function
  API, so it needs JDK 22 or later and brings no dependency: no JNA, no JNI,
  no glue of its own, and it does not go through the C API. `OfflineProtocol`,
  `MeshServices`, `DataStore`, every record and enum, the callback interfaces
  a host implements (events, transports, storage providers) and a
  `ProtocolException` subclass per engine refusal. The generated records have
  no defaults, so a `ProtocolConfig` is spelled out in full. CI regenerates
  and diffs the Java with the rest, and builds and tests it against the
  library on Linux, macOS and Windows. The generator is versioned apart from UniFFI, so
  the script refuses any series but 0.5. The jar is published to Maven
  Central as `com.offlineprotocol:offline-protocol-java` by the release, beside
  the Android library and under the same switch; it holds no native code, so the
  host supplies the library from the release's assets. The contract is
  `docs/bridges/java.md`.
- **A C API.** `bindings/c/include/offline_protocol.h` declares about a
  dozen functions over the native library every release already ships:
  create an engine from a JSON configuration, call any method of the
  interface definition by its local API name with JSON parameters
  (`offline_protocol_call`), receive events as JSON through a callback or a
  poll queue, attach storage providers written in C, and run the storage
  conformance suite against one. The method names, encodings and error
  positions are the local API's, so a method added to the definition is
  callable from C with no change to the header, and a wrapper in Go, C# or
  C++ binds a fixed set of functions. Every entry point catches a panic
  rather than aborting the host. The dispatcher is generated from the
  definition and checked for staleness; the header is generated by cbindgen
  and committed; a C program built with `-Wpedantic -Werror` links against
  the shared and the static library and runs in CI on Linux and macOS. The
  desktop release archives now carry the header, and the Windows archive
  the DLL's import library. The macOS library's install name is now
  `@rpath/liboffline_protocol_uniffi.dylib` rather than the build machine's
  path, so a host linked against it finds it through its own rpath. Every
  callback and storage provider handed to the library has its `drop`
  called exactly once, a refused registration included, so a host never
  frees its own context. Before anything publishes, the release builds
  the quick start in `bindings/c/README.md` against each desktop library
  with that archive's own link line and runs it, on macOS, Linux and
  Windows. CI also runs two engines exchanging messages through a C host,
  every C program under AddressSanitizer and UBSan, and the quick start
  under MSVC, and a nightly job fuzzes the JSON boundary. The contract is
  `docs/bridges/c.md`.
- **Signing large payloads from React Native no longer boxes every byte.**
  `signDataBytes(Uint8Array)` and `verifySignatureBytes` sit beside
  `signData` and `verifySignature`, which are unchanged. They cross the bridge
  as base64 strings, and the native modules hand the decoded buffer to new
  `sign_data_bytes` and `verify_signature_bytes` entry points in the
  interface definition, typed `bytes` rather than `sequence<u8>`. The old
  path allocated one boxed number per byte in JavaScript and one boxed
  `UByte` per byte, twice, on the Android heap, which exhausts that heap on
  a payload of tens of megabytes before anything is signed. Signatures are
  identical for the same input. The base64 codec is the SDK's own, since
  `btoa` and `atob` are missing from Hermes before React Native 0.74 and from
  JavaScriptCore. On the local API the two are platform operations: the wire
  already carries base64, and a signing twin there would reach the shared key
  past the `sign_data` deny group.
- **Large file sends from React Native no longer stage a base64 string on
  the bridge.** `sendFileFromPath({recipient, filePath, fileName})` sits
  beside `sendFile`, which is unchanged. It passes a path (or a `file://`
  URI) to the Rust core, which reads the file itself, so the Kotlin and
  Swift sides hold only the path. On constrained Android a 25 MB attachment
  through `sendFile` materialised a ~34 MB base64 JS string, a decoded byte
  array and a boxed `List<UByte>` before Rust saw it; the new entry point
  skips all three. The read is whole-file into the Rust heap, not streamed.
  A missing file, anything that is not a regular file (a FIFO would block,
  a device file would read until the process aborted) and a file over the
  100 MiB transfer limit return `InvalidArgument` before a byte is read.
  Content type is always `File`. It is reachable from every binding
  (TypeScript, Kotlin, Swift, Java, Python, C). The local API withholds it
  as a platform operation: on the wire a path would be a read of any file
  the server can open.
- **The service image is published.** The release workflow builds
  `bindings/python/docker` for linux/amd64 and linux/arm64 and pushes it to
  `ghcr.io/offline-protocol/offline-protocol-service`, tagged `X.Y.Z`, and
  `X.Y` and `latest` for a release without a prerelease suffix, with a
  provenance attestation and an SBOM. It runs after the PyPI publish and
  installs exactly that release's package: the Dockerfile's new
  `SDK_VERSION` build argument pins `offline-protocol-sdk[lan,http]` to it
  and fails the build on any other version. `service-image.yml` can also be
  run by hand, from the default branch, for a release already on PyPI, which
  is how 0.29.0 is published. It never pushes `X.Y.Z` a second time unless
  told to, for an image that failed its checks.


### Changed

- **`MessageDelivered.latency_ms` runs from the first send.** An app that shows or logs
  this value now sees the wait since a carrier first accepted the message,
  not the last attempt. On the usual path it ran from when the pending
  acknowledgement was registered, so a message re-sent after its
  acknowledgement timed out, or parked after the relay took it, reported only
  its last attempt, while a parked message settled by a neighbour reported the
  whole wait. Both paths now measure from the first transmit a carrier
  accepted; time spent before that, while every transport refused the
  message, is still not included. Transport scoring still uses the last
  attempt's round trip.
- **The iOS and Android libraries are built for size, and still unwind.** A
  new `mobile` profile, `release` at opt-level "s", builds both, in the
  release workflow, the React Native scripts and CI; the desktop libraries
  and the wheels stay on `release`. On the macOS arm64 build, with the
  link-time optimisation 0.30.0 also turns on, the shared library is
  8,538,400 bytes at "s" against 10,932,208 at opt-level 3. The cost, on the
  benches: MLS sealing and opening about 40% slower, other paths at most a
  third. Opt-level "z" was measured too and rejected: a quarter smaller
  again, but MLS twice as slow and small-message serialization three times.
  The `minisize` profile is deleted. No build ever used it, and ADR 0018,
  ADR 0019, threat model R11, the C guide and CLAUDE.md, which said the
  mobile library shipped with its `panic = "abort"`, now say what ships: every
  library unwinds, so a panic still reaches Swift and Kotlin as an exception
  and a document whose import panics in the engine's apply step is marked
  unusable rather than ending the app. A panic in the decode before it is not
  caught there and unwinds out of the frame's handling; the in-flight marker
  quarantines that blob
  ([ADR 0029](docs/adr/0029-mobile-libraries-are-sized-not-aborting.md)).
- **UniFFI is now 0.32** (was 0.30). The Swift, Kotlin and Python bindings are
  regenerated together from the unchanged interface definition, and the
  `uniffi-bindgen` the build scripts and CI install is `0.32.2`. No API a
  binding exposes changes. A checkout that regenerates bindings needs
  `cargo install uniffi --version 0.32.2 --features cli --locked --force`,
  and a React Native app rebuilds its native code to pick up the new
  generated files. Three floors move with it:
  - Building `offline-protocol-uniffi` (every binding, and the C library
    from source) and installing `uniffi-bindgen` need Rust 1.91, because
    UniFFI 0.32's build dependencies do. The library crates still build on
    Rust 1.87.
  - The generated Swift uses `nonisolated(unsafe)`, so the iOS bridge needs
    Swift 5.10 (Xcode 15.3) or newer.
  - On iOS, `ProtocolError` and `MlsStorageError` now conform to
    `LocalizedError`. An error the bridge does not map to a code reaches
    JavaScript, and the bridge's diagnostics, as the error's own case and
    message (`ProtocolError.SendFailed(message: ...)`) instead of the
    generic Foundation description.
- **`offline-protocol-uniffi` no longer builds a Rust library.** Its crate
  types are `cdylib` and `staticlib`, the two the bindings and the C API
  ship, so the crate can no longer be named as a dependency from Rust. Nothing
  in the workspace or on crates.io did. The C binding's fuzzer compiles the
  crate's source under its own package instead, which also lets it run on
  macOS.
- **The dependency audit waives RUSTSEC-2026-0374**, an unmaintained-crate
  notice (not a vulnerability) for `basic-toml`. It reaches the workspace only
  through the FFI crate's build script, beneath `uniffi_bindgen` and
  `askama_derive`, which read template configuration with it at compile time,
  so it never links into a shipped library. No upgrade removes it: uniffi
  0.32.2 and askama 0.16.1 are the latest releases. The waiver is in both
  `deny.toml` and `.cargo/audit.toml`, and is dropped once an askama release
  replaces the crate and uniffi takes it.


### Fixed

- **Telemetry counts a delivered message once, and only the application's.**
  Every acknowledged frame reported `protocol.message.delivered` to the
  telemetry pipe: each chunk of a media transfer, and internal frames such as
  connection requests, key packages, Welcomes and data-sync frames. None of
  them is ever reported as sent, so the dashboard's delivered count, success
  rate, hop count and latency were inflated against its sent count, most for
  apps that send media. The pipe now hears a delivery only for a message sent
  through the app send path (`send_message`, `forward_message`, a rich send).
  `MessageFailed` follows the same rule, so the success rate (delivered over
  delivered plus failed) is no longer lowered by internal frames that failed.
  `on_event` is unchanged: the app still receives every `MessageDelivered` and
  `MessageFailed` it did before.
- **A phone no longer loses a Bluetooth peer it is still linked to.** In the
  2026-10-19 volume test an iPhone and an Android phone held two Bluetooth LE
  links between them; the one that stalled was torn down, the iPhone's redial
  of its address timed out and reported Android lost, and the core refused
  every send to Android for ten minutes while the second link sat idle. No
  reconnect could succeed, because the radios complete one connection between
  two devices. Now iOS retires the dropped identifier instead of reporting the
  peer lost while another link to it is up, and skips dials to a peer it is
  linked to; Android closes a client link to a peer already subscribed to its
  server, and restarts its advertisement (which gives it a new address) on its
  two-minute refresh only while no identified peer is linked, or when the
  advertisement has stopped. Evicting a peer on iOS now drops every
  identifier it maps from, so a second link no longer survives a peer the
  core was told is lost. See ADR 0030.
- **A full outbox refuses the newest message instead of failing the oldest.**
  At 500 entries the outbox failed its least recently sent entry to admit any
  new one, so the same outage failed about 2,500 queued messages per phone,
  each to make room for a newer message to the same peer. Now a message to the
  recipient with the largest backlog is refused (`message_failed`, "Outbox
  capacity exceeded", no attempts, while `send` returns its id); a message to
  a recipient with a smaller backlog evicts from the largest one (its newest
  message held by the per-peer window, else its oldest entry); and a message
  awaiting its acknowledgement is never evicted.
- **A link carrying less than it is offered no longer collapses.** Two phones
  sending 5 messages a second each way over Bluetooth LE delivered 70.5%: the
  Android phone's indication path carried 12 to 16 frames a second against 15
  offered, acknowledgements queued behind data until every message timed out,
  each resend cost the receiver one more acknowledgement on the saturated
  link, and both phones retried 50,000 times and failed 6,000 messages as
  "Outbox capacity exceeded". Four changes:
  - A sender keeps at most 16 frames awaiting acknowledgement in flight to
    one peer over a mesh link. Further messages wait in the outbox, are
    reported as `message_deferred` with reason `peer_window`, and go out
    oldest first as acknowledgements return. The relay is not windowed, and
    neither is session control (key packages, Welcomes, confirmation
    probes), which heals the session the held frames are sealed under.
  - The Bluetooth LE carrier queues an acknowledgement ahead of data.
  - Delivery acknowledgements to a peer that advertises `ack_versions` entry 2
    are coalesced, up to five per frame within 500 ms, in a new `ack_more`
    metadata key. Older peers get one acknowledgement per frame.
  - On Android, the indication path sends the next fragment as soon as the
    peer confirms the last, instead of 20 ms later.
- **A resend the receiver can no longer decrypt is re-sealed.** Past the
  re-seal budget a resend went out verbatim, and a verbatim copy sealed more
  than 32 generations before the newest was refused by the receiver,
  unanswered on a mesh link, until its retries ran out. It is now re-sealed,
  up to 512 generations past the last acknowledgement.
- **A full acknowledgement table no longer stops the engine.** Once 1,000
  messages were waiting for acknowledgements, a retry with no slot returned
  an error out of `process()` before the step that frees slots, on every
  tick, and the engine never recovered: every later send failed and nothing
  more was retried. A frame is now never transmitted without a slot to track
  it. At the ceiling a send is held for the retry queue, reported as
  `message_deferred` with reason `ack_capacity`, and sent once an
  acknowledgement frees a slot; `process()` runs every step on every tick.
  Two leaks that kept the table full are fixed with it: a refused
  registration orphaned every timed-out entry so it was never retried, and
  the prune of timed-out entries ran at exactly the retry ladder's maximum
  delay, so a ninth retry found its slot gone.
- **`sendMessage` no longer reports a failure for a message it sent.** The
  capacity error used to surface after the frame was on the wire, so an
  application showed messages as failed that the recipient had received.
- **A receiver that falls far behind a sender re-keys instead of going
  deaf.** A frame more than the forward window ahead of the receiver's
  ratchet failed as an ordinary decryption error, and so did every later
  frame and every resend from that sender, permanently. It now triggers the
  same rate-limited re-key as an epoch desync. The trigger is authenticated:
  it needs the epoch's sender-data secret. The window is also raised from
  1,000 to 8,192 generations, pinned at four times the in-flight ceiling.
- **A frame still waiting in the BLE queue is not retried.** Its
  acknowledgement timeout is extended, for up to a minute, instead of
  queueing a second re-sealed copy behind it. On a link carrying less than it
  was offered, those copies grew the queue, the round trip and the
  receiver's ratchet gap together.
- **Large BLE messages are no longer torn under load.** The Android and iOS
  outbound fragment queues discarded everything queued for a peer when they
  filled, and they could fill: Android's periodic poll pulled fragments from
  the core with no backpressure check, and both drains checked only after a
  fragment had been pulled. With a cap of 100, one 16 KB message was enough.
  Both drains now stop before pulling while any peer with a link is backed
  up, and the cap is 768, above the largest message the core fragments. A
  peer with no link does not stop the pull, so its undeliverable fragments
  no longer hold back every other peer's.
- **Resends stop burning encryption keys while nothing is acknowledged.**
  Each resend used to be re-sealed under a fresh ratchet generation, so a
  link that stopped carrying acknowledgements walked the receiver past its
  forward window: one phone re-sealed 20,919 resends during a 17-minute
  stall. A sender now re-seals only while it has sealed fewer than 256
  frames past the peer's last acknowledgement, and resends verbatim beyond
  that until the next one arrives. 256 is a quarter of 1,024, the receiver
  window of a leaf node, which mls-rs fixes and which is smaller than a
  phone's; `tools/mls-interop` pins it.
- **An iOS central link whose write buffer never drains is torn down and
  reconnected.** CoreBluetooth kept refusing every write on such a link for
  as long as it lived while still delivering the peer's notifications, so
  nothing reported it down. After 15 s with no accepted write the link is
  cancelled. A connect that has not completed in 10 s in the foreground is
  cancelled too: CoreBluetooth never times one out, and the pending connect
  refused every reconnect attempt as "already connecting".
- **A message sent while its sender was blocked is never delivered.** Its id
  is now remembered, so a retry arriving after an unblock is answered rather
  than delivered.
- **An iOS phone keeps talking to a peer that changed its Bluetooth
  address.** The peer reappears under a new identifier, and the old one still
  mapped to it, so the send path could pick the old, disconnected one and
  report "no connected peripheral" indefinitely while the verified link to
  the same peer sat idle: 25 minutes in one test. Sends now go to the
  connected link, verifying a peer retires its other disconnected
  identifiers, and the connection monitor stops dialling a peripheral it
  never identified once it stops advertising.
- **The BLE bridges stop pulling far ahead of the link.** Both outbound
  fragment queues pulled from the core until a peer held 576 fragments, so
  the core saw nothing queued, could not extend the acknowledgement timeout
  of what was still waiting, and resent it: 19,404 resends from one Android
  phone on an indication-paced link. They now stop at 32 fragments, a few
  seconds of link time, and the rest waits in the core where the extension
  sees it. The 768 cap is unchanged.
- **Link-time optimisation reaches the libraries the release ships.** The
  release profile has always asked for it, but cargo runs it on a library
  only when every crate type in the library's list can take it, and the FFI
  crate also listed `lib`, which cannot. So every native library from the
  UniFFI migration to 0.29.0 (the iOS framework, the Android libraries, the
  desktop libraries and the Python wheels) was built without it, and no
  build said so. Measured on the macOS arm64 build: the static library the
  iOS framework is made from falls from 137,949,536 to 23,393,288 bytes, the
  shared library from 11,534,416 to 10,932,208, with the same exported
  functions. The npm package carries the iOS static libraries, so it should
  lose most of its size; the iOS and Android numbers are measured on the
  first release artifacts built this way. A release build of the crate takes about a
  minute longer and peaks near 2.1 GB of memory.
  `the_ffi_crate_builds_no_rust_library` refuses a crate type that would
  turn it off again.
- **Python: type checkers read the package's annotations.** The package
  shipped no PEP 561 `py.typed` marker, so mypy skipped it as untyped
  ("missing library stubs or py.typed marker") and every value it returned
  was `Any` in the application, although nearly every public function is
  annotated. The wheel now carries the marker, and the wheel build fails
  if it goes missing. The root package also names its public surface in
  `__all__`: under PEP 561 a type checker treats a bare `from .x import Y`
  in `__init__.py` as private, so with the marker alone pyright (and so
  Pylance) refused the README's own `from offline_protocol_sdk import
  ProtocolManager`, and `mypy --strict` reported that the module "does not
  explicitly export" it. A test pins the list to the attributes the package
  exposes, and the wheel harness refuses a wheel without the marker.
- **Python: a Linux box that could no longer advertise recovers on its own.**
  Linux loses track of the controller's advertising when a central-role
  connection completes while advertising is on: the controller keeps
  advertising, the kernel believes it stopped, and every later advertisement
  is refused with "Failed to register advertisement" until the controller is
  reset, across service restarts. When BlueZ refuses an advertisement that
  way while reporting free slots, the peripheral now powers the adapter off
  and on once (which resets the controller) and starts again, and both roles
  register again whenever an adapter powers back on.
- **Python: Bluetooth LE no longer takes bluetoothd down, and survives it
  restarting.** On BlueZ 5.87 bluetoothd exits the moment a
  discovery filter naming a service UUID is set, and the central set one at
  every start, so each start of the service ended the advertisement and the
  scan within about half a second while both roles went on logging themselves
  running. Two Linux boxes never formed a Bluetooth LE link, and a box dropped
  off the air for a phone. On Linux the central now scans unfiltered and
  filters on the mesh service UUID itself, and both roles watch for
  bluetoothd restarting: a start cut short by bluetoothd exiting is retried,
  and a running role registers again with the new bluetoothd.
- **One lost inbound Bluetooth fragment no longer marks BLE as a failing
  link.** When a partial message idled out of the reassembly cache or was
  evicted at capacity, the BLE transport counted it as a failed send, and
  nothing ever counted a successful one, so BLE's delivery ratio read 0% for
  the life of the process. The transport manager then ignored the engine's
  own acknowledgements for BLE, because it only used them for a transport
  reporting no counts of its own, and DORS took the low-success-rate
  emergency switch within a few sends, bypassing hysteresis and cooldown
  where Wi-Fi Direct was available. `get_transport_metrics` reported the same
  0% to the application. A reassembly drop is no longer a delivery sample,
  and BLE and Wi-Fi Direct always take their delivery ratio from
  acknowledgements; Internet, Reticulum and Nostr keep the counts their
  confirmation loops report.
- **DORS needs five delivery observations, not five sends, before the
  low-success-rate rule applies.** The selector took a success-ratio sample
  from the transport's cumulative counts on every selection, so one early
  failure became five identical 0.0 samples after five sends and met
  `min_ble_samples_before_success_rate_escalation` with no new evidence. It
  now samples only when the delivery counts change.
- **React Native recovers an event lost on the way to JavaScript instead of
  only reporting it.** A device test saw a sender get a receipt for a
  message whose recipient app never got `message_received`, and a sender
  whose app never got `message_delivered` although the message stopped
  retrying. In both cases the core had already acted: the message was
  acknowledged and its id marked, or its outbox entry and retry removed, so
  nothing would ever restate the event. Each native module now keeps the
  last 256 events it handed over, by the `seq` number 0.28.0 added, and
  serves them through a new `replayEvents` bridge method. When a number is
  skipped, the TypeScript layer pauses dispatch, fetches the missing events,
  and delivers them in their place, reporting the recovery as a `diagnostic`
  with `message: "native_event_replayed"`. `native_event_gap` now reports
  only what the module no longer held. Three further ways to lose one of
  these events silently are closed. `message_delivered` and `message_failed`
  are held for redelivery when JavaScript cannot take them, as
  `message_received` already was. A poisoned shared-state lock no longer
  skips every later event emission. And an exception thrown inside the
  Android module's event callback is caught and logged by event type, where
  it used to become a panic in the core that only a filtered log target
  recorded.


### Removed

- **`BleTransport::record_send_success` and `record_send_failure`.** They had
  no caller outside their own test, and the failure half was the cause of the
  fix above. BLE has no send confirmation loop to feed them.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
