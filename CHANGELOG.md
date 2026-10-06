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

- **Android forms its Wi-Fi Direct group itself.** With
  `wifiDirect: { enabled: true, autoAccept: true }` on Android 10 and later,
  devices of the same app find each other over Wi-Fi P2P service discovery
  (`_offlineprotocol._tcp`, the stream chapter's record plus an `app` entry),
  and join one group whose name and passphrase are derived from the app id:
  the lowest address creates it and the others join. No system dialog appears
  on either phone, which a `WifiP2pManager.connect` invitation would show.
  Discovery between phones is asymmetric (a device that owns or is joining a
  group answers no discovery query), so nothing depends on hearing the owner:
  a device that heard a lower peer creates the group itself after three joins
  found none, a device that heard nothing probes a group owner it sees at most
  once a minute, and an owner whose group stays empty for 30 to 60 seconds
  dissolves it so two groups that formed at once merge. A client whose owner
  proves nothing for three dials at the top of its redial ladder (an owner
  whose app died: the group can outlive the process) leaves the group and
  remembers that owner for five minutes. Every group of an app has one name,
  so a join can land on the owner just left, and Android offers an app no way
  to steer it elsewhere; a join that lands there leaves at once and counts as
  one that found no group, so a device that heard a peer creates the group
  itself after three. Which owner a join lands on beside a dead one stays the
  system's choice, so the merge there is bounded, not guaranteed. `stop()`
  removes an app-named group this device owns even when it was adopted at
  start. A device alone backs off: discovery from 15 to 60 seconds, the owner
  probe from one to four minutes. Two phones (Android 13 and 15) next to two
  Wi-Fi Direct televisions formed the group in all eight clean starts, in 18 to 225 seconds (median
  about a minute). Off by default; without it a group is formed in the
  system's Wi-Fi Direct settings, as before, and that group is never
  dissolved. **An app that already sets `autoAccept: true`**, as the
  integration guide's example did while the option did nothing, starts forming
  groups on upgrade; set it to `false` to keep the old behaviour. The group's
  passphrase is derived from the app id and is not a secret: it keeps apps
  apart, and the identity preamble and end-to-end encryption still protect the
  traffic (threat model R22). `groupOwnerIntent` is not used and is
  deprecated. Groups of three or more devices are untested.

- **Python has a gateway-daemon client.** `GatewayManager`, as
  `ProtocolManager.gateway` when `reticulum_enabled=True`, speaks the
  [gateway-daemon contract](docs/spec/gateway-contract.md) over TCP to a
  daemon on local IP: it attaches with the core-built address declaration,
  offers the carrier to the core only once the gateway has bound the session
  to this device's address (its capabilities are handed to the core on their
  own frame, which the contract puts before the announcement), settles every
  send on the gateway's verdict rather than on the socket write, fails what a
  dead or silent connection owes (sixty seconds, under the core's own expiry),
  and asks about the core's presence watchlist. It is the first client on this
  carrier to read the `stored` and `pushed` verdict flags, reported as
  `relay_stored`, `relay_pushed` and `relay_pushed_stored`, the relay client's
  mapping. The stub callback the manager used to install for this slot is
  gone; an application that drives the slot itself still replaces the callback
  the same way. `configure(daemon_address=...)` and `start()` it after
  `ProtocolManager.start()`; `stop()` stops it with the rest. The decisions
  live in `gateway_attach_policy.py`, `gateway_verdict_tracker.py` and
  `presence_watch_policy.py`, ports of the Swift files, with their
  hand-mirrored constants pinned as literals and read by the Rust guards
  beside the Swift and Kotlin copies (P11 in `docs/bridges/python.md`). No
  daemon has been run against it.

- **A local API guide and two client examples.** `docs/local-api.md` is
  the guide to running the SDK as a service several local applications
  share; `bindings/python/examples/local_api_client.py` (Python) and
  `examples/local-api/client.mjs` (Node 22, no dependencies) each show
  `hello`, `subscribe`, a send with its delivery event, a document edit read
  back, and the one-shot idiom. The Python suite runs both against an
  in-process server, the Node one when `node` is on the path.
- **The Python reference server for the local API.**
  `offline_protocol_sdk.local_api` and the `offline-protocol-service` command
  front one engine for any number of local applications over JSON-RPC 2.0 on
  a WebSocket, on an owner-only Unix domain socket by default or on loopback
  TCP with a per-launch token. The server owns the run loop and the drain;
  a client declares its application id once in `hello`, every send is
  stamped with it, and every event is relayed as the engine serialised it to
  the clients the chapter's rules select (by application id, by an
  identifier the server issued, or to everyone), with the stamped inbound
  events held for an application whose client is away. The method table is
  generated from the interface definition and checked in
  (`local_api/table.py`), every declaration is classified as exposed or
  platform-only in `dispatch.py`, and a Rust guard in the FFI crate holds
  the chapter, the definition and that classification to one another, so
  an unclassified method is a failing test rather than a method every local
  application can reach. Optional rules from one JSON file: a space
  allow-list and method denials per application id, which once configured
  refuse a `hello` under an id no rule names. A test over two servers on
  one host exchanges a message over the peer-stream transport.
- **The local API chapter.** `docs/spec/local-api.md` specifies how one
  server process fronts one engine for several local applications: JSON-RPC
  2.0 over a WebSocket on a Unix domain socket by default (TCP on loopback
  with a per-launch token as the opt-in), a `hello` that declares the
  client's application id once and stamps it on every send, and the events
  relayed unchanged as notifications. It is the first complete catalogue of
  the engine's events: all seventy-five tags with their fields and the
  vocabularies of the enum-valued ones, and how each is routed (by
  application id, by an identifier the server issued, or to everyone). The
  method table partitions the interface definition into the 127 methods a
  client may call and the 93 platform operations it never can, including the
  run loop and the drain, which the server owns because the engine delivers
  a message only when something drains. Errors keep the engine's
  twenty-five-variant taxonomy: the JSON-RPC code is the variant's position
  in the append-only enum. There is no HTTP request path: the pinned
  WebSocket library drops any non-`GET` handshake without a response, which
  the chapter records so it is not re-added. `docs/bridges/local-api.md`
  states which shared bridge rules the server inherits and the new rule that
  a Rust guard pins the three tables to the definition, the engine and the
  reference server. Nothing ships in this entry but the contract; the
  reference server follows it.

- **Custody v1.** A device can now hold a neighbour's replication frames for
  hours instead of the five seconds a forwarder gives them
  (`docs/spec/custody.md`), off by default. `ProtocolConfig::custody`
  (`custody` in every binding) switches it on and sets the hold and the
  quotas; the hold is validated strictly shorter than the outbox lifetime. The
  depositor's engine writes the one-hop request on its own sealed `delta`,
  `snap`, `vv` and `blob_gone` frames when it offers them to neighbours, from
  the plaintext it retains for re-sealing and never after a restart; every
  forwarder strips the request from a third-party frame it transmits. A
  custodian judges a frame at the drop point, after the forwarding identifier
  is released, so it never blanks its own route; answers a depositor that
  advertises `data_versions` entry 7 with the signed `__CUSTODY_RECEIPT__`
  once, over the arrival link; redelivers on neighbour discovery through a
  dedicated governor intake, at most once per neighbour per hold and straight
  to the recipient when it appears; expires records in wall time against the
  hold in force; and keeps them sealed under the new `custody_entries` storage
  category, restored at launch. A receipt settles nothing. `custody_stats()`
  (`get_custody_stats()` over the FFI) reports the counters, every refusal
  reason included, and `erase_custody()` drops every record; the data-layer
  wipe calls it. The receipt body has frozen vectors at
  `crates/offline-protocol/tests/data/custody-receipt-v1.vectors.json`.

- **The custody chapter.** `docs/spec/custody.md` specifies how a device
  holds a neighbour's replication frame for hours instead of the five
  seconds a forwarder gives it today: an explicit deposit in which the
  depositor asserts the class and this version carries only `delta`, `snap`,
  `vv` and `blob_gone`; a signed receipt that settles nothing; a hold that is
  validated strictly shorter than the outbox lifetime, because a custodian
  holds ciphertext it cannot re-seal; acceptance only for what the mesh could
  not forward, at the point where the forwarding identifier is already
  released, so a custodian never blanks its own route; redelivery as an
  ordinary forward; entry and byte quotas per depositor with a stranger tier
  of zero; and an erase of its own, because no global wipe exists. The
  control-message registry and the engine's prefix list reserve
  `__CUSTODY_RECEIPT__`, the wire-format chapter reserves the metadata key
  `__custody`, and `data_versions` gains entry 7 for the receipt. The threat
  model gains R19 (custody-borne re-key pressure), R20 (a custodian retains
  third-party routing metadata) and R21 (deposit spam). Custody v1, above,
  implements it.

- **The leaf node builds for ESP32 RISC-V parts and for Cortex-M0.**
  `offline-protocol-leaf` now compiles, and CI lints it, for
  `riscv32imac-unknown-none-elf` (ESP32-C6, ESP32-H2),
  `riscv32imc-unknown-none-elf` (ESP32-C3, ESP32-C2) and `thumbv6m-none-eabi`
  (every Cortex-M0), beside the Cortex-M33. The last two have no atomic
  compare-and-swap, so a firmware on them must link a critical-section
  implementation. The crate exports `SharedStore` and `shared_store` for the
  store handle; where atomics exist `SharedStore` is `Arc<dyn LeafStore>`, so
  existing code is unchanged. This is a compile result only: nothing has run
  on an Espressif board, the Xtensa chips were not tried, and ESP32-C3 and
  ESP32-C2 cannot yet be built on esp-hal, because mls-rs and esp-hal select
  portable-atomic backends that refuse to coexist.
- **A send can name the application it is for.** `SendMessageOptions` and
  `MediaSendOptions` take an optional `app_id` (React Native: `appId` on
  `sendMessage` and `sendMedia`), stamped on that message or on every chunk of
  that transfer in place of the configured one. It is for one instance
  serving several local applications behind one identity. It changes the
  stamp on that frame only, never the storage namespace, the MLS session or
  the BLE app tag, and the acknowledgement for it keeps the configured id. The
  id survives the pending queue, the media pump and a restart, and an invalid
  one fails the call with `InvalidArgument` before anything is queued.
- **Received events say which application a message was for.**
  `message_received` carries `app_id` on both the live and the delayed-decrypt
  path, `file_received` carries it from the transfer's first chunk, and
  `media_resend_required` names the per-send id of an interrupted transfer so
  the right application is asked for the bytes. The id is cleartext and
  unsigned on the wire, so route on it and never authorize on it; the
  threat model records this as residual risk R17.
- **The MLS storage trait has a conformance suite.**
  `offline_protocol::mls_storage_conformance::run` (and `run_json`) holds an
  `MlsStorage` implementation to the contract the engine relies on, in the
  same report shape as the protocol-state suite: the round trip, overwrite,
  absent-key and delete semantics, key-type isolation, listing, `exists` and
  `clear_type`, key ids as the SDK writes them including the `:` in a session
  id, key ids compared byte for byte (group ids are peer-chosen, so a
  case-folding backend merges two groups), a group id at its 4096-byte cap, a
  512 KiB record, and overwrites from several threads landing whole. It
  writes only under its own probe key types, checks key-type isolation before
  any delete and stops if it fails, and deletes everything it wrote. Probe a
  new backend on a scratch instance until it is green. Green is not a
  persistence test. Rust only; it is not exposed over the FFI.
- **Built-in file stores for a host without a platform keystore.**
  `FileProtocolStateStorage` writes protocol state in the `OPS1` format the
  iOS, Android and Python providers write, byte for byte, so a state
  directory opens under any binding for the same account.
  `SealedFileMlsStorage` keeps MLS material sealed with ChaCha20-Poly1305
  under keys derived from an operator-supplied store key and the account
  namespace, with record keys inside the ciphertext and file names under a
  keyed digest. The store key comes from a `StoreKeyProvider`: `EnvStoreKey`
  (`OFFLINE_PROTOCOL_STORE_KEY`, hex or base64) or `StaticStoreKey`. A wrong
  key is refused at open with `FileStoreError::WrongStoreKey` before any
  record is read, and a lost or damaged key check is rebuilt only when a
  record proves the key; a damaged check over an empty store fails with
  `FileStoreError::KeyCheckUnverifiable`, naming the file to remove. The
  sealed store never deletes a record on a read; a damaged one is reported
  as `CorruptedData` naming its file, and one the listing skips is logged
  as a warning naming its file, once until that file is written or removed.
  A record that cannot be read is reported as a read failure, never as one
  sealed under another key, and never counts towards refusing the store key. Each store holds an
  exclusive lock on its directory while open, so a second engine over the
  same directories fails with `FileStoreError::InUse` instead of silently
  diverging the MLS state. `close()` on either store releases its directory
  at once, whoever still holds the store, and every later operation on it
  is an error (a failed operation, never a lost record).
  `FileStorePair::open` opens the two stores together and refuses the
  pairs the engine would lose data over (see the next entry); its first open
  binds the two with one pairing id, so a later open knows they belong
  together whatever the engine does to its record key.
  `FileProtocolStateStorage::sealed_state` says whether the sealed records
  in a state store open under the record key an MLS store holds, without
  changing either: `Empty`, `Opens`, `Foreign` (sealed under another key,
  or under one this identity lost and replaced) or `KeyDamaged` (the record
  key itself is damaged, which no refusal helps).
  On Windows the lock file shares read access, so a backup tool that shares
  write access, as they usually do, can read a live store. A failed
  directory flush fails the write on Unix rather than acknowledging it, and
  a store opens over a relative root on its first run.
  `account_storage_namespace` is the binding-shared namespace
  derivation. Both stores pass their conformance suites, and an
  engine restarted over them keeps its address and its sealed documents. The
  `replicated_notes` example now runs on them. The stores sit behind the
  default-on `file-store` feature. The threat model records the store key's
  exposure on a host without a keystore as residual risk R18.
- **Bindings can open the built-in file stores.**
  `initialize_mls_with_file_stores(mls_root, state_root, store_key)` opens
  both stores in place of `initialize_mls`, in the account directory derived
  from the instance's own `app_id` and `profile`. It keeps `initialize_mls`'s
  lifecycle, and its refusals use existing error variants
  (`InvalidArgument`, `InvalidConfiguration` for a wrong key, `InvalidState`
  for a directory another store holds), so the error enum is unchanged. The
  Python `ProtocolManager` takes `store_key=` or `store_key_env=` (64 hex
  digits or base64, as the Rust `EnvStoreKey` reads), with `mls_root=` or
  `OFFLINE_PROTOCOL_MLS_ROOT`, and uses the file stores instead of the
  keyring. On that path `start()` raises when MLS cannot be initialised,
  rather than logging and starting without the identity, and releases the
  callbacks it registered. Swift and Kotlin get the generated methods and
  keep their platform keystores.
- **The file-store entry point refuses a pair of roots it would lose data
  over.** The engine deletes a sealed protocol-state record that does not
  open, which is right for one damaged record and wrong for a whole state
  root sealed under another identity. Moving onto the file stores starts a
  new identity, so each of these is refused, and no refusal changes or
  deletes a record: a state root that holds records with no MLS record
  beside it (asked of record files, so neither an earlier attempt that
  failed nor a first write that failed can disarm it); a state root that
  holds records and belongs with another MLS store (an identity is in
  place, but not the one that wrote the state), decided by the pairing ids
  the two stores' first open wrote, and for a state root never bound, or
  bound to another id, by whether its sealed records open under this MLS
  store's record key, which only this identity's do; an
  account directory that cannot be read, which is never taken for empty;
  and roots that are one directory
  or one inside the other, compared as spelled before anything is created
  and again as directories once both exist, because a volume that folds
  case makes one directory of two spellings. A damaged record key is not
  refused, on the launch that finds it or on any launch after while the MLS
  root keeps its pairing file: the engine's
  new key is what settles the messages lost with it, and the records it
  cannot delete that launch stay under the lost key without making the
  state look like another identity's. Rust hosts get the same refusals
  from `FileStorePair::open`.
- **`close_file_stores()` releases the file stores without waiting for the
  instance to be freed.** When a host object is freed is the host runtime's
  decision: a kept reference, a callback cycle, an exception that still
  names the object. Until then the directories stayed locked and every
  later instance over them was refused. The new FFI method releases both
  locks at once, after `stop()` (or after a panic poisoned the engine
  lock, which `stop()` cannot take again), and stops telemetry first so
  the uploader's final flush reaches its queue. The instance cannot run
  again afterwards, and it is a no-op on an instance that never opened the
  file stores. The Python `ProtocolManager` gets `close()`, which stops
  and then releases, and leaving `async with` closes a manager that uses
  the file stores, so two blocks over the same directories need no `del`
  between them; a block whose entry failed after the stores opened closes
  them too. Any other manager is stopped by the block, as before, and can
  be entered again. A `close()` cancelled while the core is releasing
  still releases, and the manager is closed once it has.
- **A Swift package and an Android library, built without React Native.**
  Both are built from the bridge sources the React Native module compiles,
  where they are: the transport managers, the storage providers and the
  generated bindings, less the five files that need React. The Swift package
  is `OfflineProtocolSDK`, assembled by `scripts/assemble-swift-package.sh`.
  The Android library is `com.offlineprotocol:offline-protocol-sdk`,
  built by the Gradle build in `bindings/kotlin`. CI builds and tests both
  on every pull request, and builds an application against each. A release
  publishes them (below). What is public in them is what the React Native
  module happened to need public, not a chosen API, and on iOS that leaves out
  the storage providers, so an application cannot construct the built-in
  stores there. Both carry the license, the commercial license, the
  third-party notices and the export notice.
- **A release publishes the Swift package, the Android library and the
  Python wheels.** Each is built from the release's own libraries and tested
  before anything publishes: the Swift package on a simulator with an
  application built against it, the Android library with all four ABIs and a
  minified application, and each wheel installed by pip and run through the
  whole Python suite on macOS, Linux (x86_64 and aarch64) and Windows. A
  failure stops npm and crates.io as well. The wheels and the Swift package's
  XCFramework (`offline-protocol-X.Y.Z-swiftpm-xcframework.zip`) are attached
  to the GitHub release, and so is the assembled Swift package
  (`offline-protocol-X.Y.Z-swift-package.tar.gz`), whose manifest names that
  archive by url and checksum. Then, each behind a repository variable, the
  library is signed and uploaded to Maven Central as
  `com.offlineprotocol:offline-protocol-sdk`, and the wheels go to PyPI by
  trusted publishing; a dry run with Maven Central on uploads a deployment
  the Portal validates and then drops. The Swift package is pulled rather
  than pushed: `Offline-Protocol/offline-protocol-swift` has a workflow that
  downloads the package and the archive, verifies their attestations, and
  commits and tags the package `X.Y.Z` with its own token, so no credential
  for it exists in this repository. The wheels used to be built
  as `py3-none-any`, four files with one name and a different library inside;
  each is now tagged for its platform, the Linux ones `manylinux_2_N` for the
  newest glibc symbol their library needs (2.34 as of this release: Debian 12,
  Ubuntu 22.04, RHEL 9 or newer; the old `manylinux_2_17` claim was never
  true of the library), and numbered from the tag in
  Python's spelling, so `v0.28.0-rc.1` builds `0.28.0rc1` rather than taking
  `0.28.0`. See [Cutting a Release](CONTRIBUTING.md#the-native-packages-and-pypi).
- **Three iOS suites run for the first time.** The mesh controller, the BLE
  discovery bootstrap policy and the error mapping suites are excluded from
  the SwiftPM test harness, and nothing else ran them. They run in the Swift
  package's job. Two mesh controller tests were failing: they registered two
  peers in a mesh with room for four, so the eviction they assert was never
  weighed. They now fill the mesh, as their Kotlin twins have since #120.
- **The leaf node builds for WASI, and CI gates it.** `offline-protocol-leaf`
  compiles for `wasm32-wasip1` in both halves: without default features,
  where `getrandom` reads the runtime's `random_get` and the firmware-style
  `bare-metal-rng` feature is not used, and with `std` through the new
  `wasi_host_shim` example, which drives a device from a runtime over its
  standard streams with the time and the frames supplied by the host. The
  claim is WASI only: `wasm32-unknown-unknown` is not gated, because there
  the pinned MLS library enables `getrandom`'s `js` feature ahead of any
  host-registered backend, so a green build on that target proves nothing
  about a non-browser host. This is a compile result plus a native run of
  the shim; nothing has run under a WebAssembly runtime.

- **Services on the LAN, and the first Python service wrappers.** A new
  specification chapter, `docs/spec/dns-sd-mapping.md`, lays a
  `ServiceDescriptor` out as a DNS-SD instance under the subtype
  `_svc._sub._offlineprotocol._tcp`: a digest instance name, a TXT record
  (`txtvers`, `sid`, `ver`, `addr`, one `c.<key>` per capability) and its
  bounds (a service id over 200 bytes, a record over 1300 bytes or a
  capability key DNS-SD cannot carry is refused, never truncated). An
  imported LAN record is unsigned: it arrives as a `service_discovered`
  event with `source: "lan"`, is kept in an application-level registry, and
  is never registered with the engine, because a registration made from it
  would go out in signed discovery responses under this node's identity.
  A peer-stream browser now ignores a record carrying `sid`, so a published
  service is not one more connector to the same host. In Python,
  `services.Services` wraps the generated `MeshServices` with the copy of
  this node's registrations the engine cannot enumerate, and refuses a
  response status outside the engine's closed set with the reason;
  `dnssd_bridge.DnsSdBridge` publishes those registrations and imports the
  LAN's, over the existing optional `lan` extra, re-resolving an import at
  half its time to live and dropping it at the whole. The service discovery
  guide is corrected where it disagreed with the engine: discovery responses
  go to the peer the query came from and are forwarded toward the
  originator, the response status is one of exactly three values, the
  version is opaque, and the peer-tracking hook is `on_neighbor_discovered`.

### Fixed

- **Confirmation probes no longer crowd real messages out of the outbox.** A
  session the peer never confirms is probed every five seconds, and each probe
  used to enter the outbox with its own retry ladder on top of the unanswered
  ones before it. Only a relay `unreachable` verdict backed that off, so a
  relay that pushes to offline users, or any mesh-only link, never did. An
  account with ten such contacts stacked about two probes a second, and once
  the outbox hit its 500-entry cap, eviction failed the user's own messages
  with `Outbox capacity exceeded`. Seen on a device test between a Samsung and
  an iPhone, offline over BLE. Each new probe now supersedes the last, quietly
  and without counting against the carrier, so a peer holds one probe in the
  outbox and is still probed on the same cadence.

- **A key package refused by this device's clock is reported.** A peer's key
  package is valid from an hour before it was minted until 30 days after,
  judged by the receiver's clock. A device more than an hour behind a peer
  cannot start a session with it, and past 30 days neither side can, so no
  session forms and messages and connection requests stay pending forever.
  The only trace was a debug line. Two Android phones that had never been
  online (one set to 2024, the other to 2025) reproduced it. The refusal
  now raises a `security_warning` with the new code
  `KEY_PACKAGE_OUTSIDE_VALIDITY_WINDOW`, once per peer, and is logged at
  `warn`, whether the package was refused on arrival or on the first send
  after a restart. `MlsError::KeyPackageOutsideValidityWindow { expired }`
  replaces the `InvalidKeyPackage` text for this one refusal on every route
  that admits a peer's package. A package whose window has already closed is
  discarded like any expired package, because it may be an old package a relay
  held rather than a clock, and kept it failed every attempt until its cached
  expiry; one whose window has not started is kept. The window itself is
  unchanged. TypeScript's
  `SecurityWarningCode` union gains the code, so an exhaustive `switch` over it
  needs a new arm.

- **An old message no longer comes back as a push notification, again and
  again.** A direct message the relay had pushed stayed in the outbox well past
  a week, because each probe re-send refreshed its lifetime and so did each
  restart. Every time the relay's push claim lapsed, the recipient got another
  notification. And because the recipient forgot message ids after a day, the
  re-send it then received failed to decrypt and was never acknowledged, so it
  came back on every reconnect. A pushed message is now dropped 7 days after
  its first send, with no event, since the push may have delivered it.
  Recipients now remember ids for 7 days by default, so a re-send inside that
  window is answered as a duplicate and acknowledged, which settles the sender.
  An acknowledgement owed when no carrier was up (a frame from a push arriving
  before the relay socket authenticated) used to be dropped. It is now held,
  for up to 10 minutes, and sent when a carrier comes up.

- **BLE survives an iPhone's Bluetooth being turned off and on, on both ends
  of the link.** Turning an iPhone's Bluetooth off and on, or a Bluetooth
  stack reset, left the pair unable to talk. On iOS, power-off removes the
  GATT service, but the bridge still marked it as published, so power-on
  advertised a service UUID with nothing behind it and peers connected and
  dropped. The service is now published again on power-on, after clearing
  whatever the stack kept, so it never appears twice. Power-on also never
  reported BLE as available again, so the core stopped sending over BLE
  while inbound still arrived. Power-off and reset now drop every per-link
  record, because CoreBluetooth invalidates the links without a disconnect
  callback for each one. That includes the mesh controller's connection
  count: a device that was at its connection limit came back with no free
  slots and refused its returning peers. Each dropped peer is also reported
  lost, so one that does not come back produces `neighbor_lost` instead of
  staying a neighbor in the core. Power-on brings the transport back to
  running, and `stop()` while Bluetooth is off now stops it instead of
  returning early and letting the next power-on restart a stopped transport.
  On the Android end, an iPhone that came back from a new random address was
  verified there, but the old address kept being redialled. When the
  redialling gave up, or the old address's GATT server link dropped
  uncleanly, it reported the live peer as lost and deleted the mapping that
  pointed at the new link. Once the peer is live at another address, the old
  address is now dropped on its own, with only its address-keyed state. This
  does not add handling for an Android phone's own Bluetooth being turned
  off and on.

- **React Native holds an interest declared before `start()` and applies it
  before the engine starts.** (#472) The engine's start-up exchange offers
  every held space with the interest in force at that moment, and a narrowing
  never deletes what a wider exchange pulled in, so interest has to be
  declared after storage opens and before the engine starts. React Native's
  `start()` does both steps, and the JSDoc said to call `setInterest` before
  it, where both native modules rejected it with `DataStore not initialized`.
  An app that moved the call after `start()` had its first exchange ask for
  everything. `DataStore.setInterest` now holds a call made before `start()`
  and resolves it, and `start()` applies what is held after MLS
  initialization and before the native start. A pattern refused then rejects
  `start()` with an error naming the space and carrying the native code, and
  the engine is not started, because starting would replicate the space
  whole; when the data layer is off the held call is dropped with a warning
  and `start()` proceeds. `destroy()` discards what is held. The behaviour
  change for a deployed app: an invalid pattern declared before `start()`
  used to reject that call and now rejects `start()`; a call after `start()`
  is unchanged. Both native `destroy()` paths now also release the data
  store, which holds a strong reference to the engine: left set, every data
  call after `destroy()` reached the stopped engine instead of rejecting, and
  on Android the engine stayed alive until the collector reached it. Python's
  `ProtocolManager` initializes MLS inside its own `start()` too; its first
  exchange still runs under the default interest, now stated in
  `docs/bridges/python.md`.

- **`DiagnosticEvent.level` declares `debug`, which both native platforms
  emit, and Android no longer emits `warn`.** (#471) The iOS and Android
  managers emit diagnostics at `debug` at about seventy sites, while the
  TypeScript union said `'info' | 'warning' | 'error'`. Android's GATT server
  also emitted four diagnostics as `warn`, a spelling nothing declared; they
  are `warning` now. The union is `'debug' | 'info' | 'warning' | 'error'`, and
  `react_native_diagnostic_levels_match_every_bridge` reads every level
  literal the bridges hand to a diagnostic sink and fails when a level is
  emitted but not declared, or declared but never emitted. This widens a
  public type to match what already arrives at runtime. It is not
  automatically source-compatible: a consumer with an exhaustive `switch` or
  a `Record` keyed by the level needs a `debug` entry to typecheck, and a
  filter that compared against `'warn'` for the four GATT server diagnostics
  should compare against `'warning'`.

- **Python holds a pinned copy of the relay-answer prefixes, and decides
  attribution in one place.** (#368) The relay's group answers reach the core
  only when they carry no transport peer identity, so an answer injected with
  a `sender_id` is refused as unsigned while the inject reports success. The
  Swift and Kotlin bridges each hold the list and force those answers
  unattributed centrally; Python held no copy, and every call site had to get
  it right on its own. 0.27.0 already corrected the prefix names and the
  attribution at each site (#453). Now `relay_answer_prefixes.py` holds the
  list, `_inject_group_frame` drops the actor for any answer whatever the
  caller passed, and `test_relay_answer_prefixes.py` pins the six literals,
  so the next registry change fails the Python suite rather than a relay
  feature in the field. No behaviour change for a correct caller.

- **A key package the application marks synced keeps its record, so its
  private key is still destroyed when it expires.**
  `mls_mark_key_package_synced` deleted the record and left the init key in
  the MLS provider. Keeping the key was right, because a Welcome built from
  the uploaded copy has to open, but the record is the only thing that carries
  expiry, so a published package nobody claimed kept its key for the life of
  the install. The mark now sets `synced` on the record instead. The package is
  withdrawn at expiry and its key destroyed a week later like any other, and
  it is never handed to a peer, because a stranger may already hold it.
  `mls_get_pending_key_packages` now lists only packages the application may
  publish: it used to include packages the engine had pushed to a peer or held
  in its own Nostr slots, which the documented upload loop would then have
  marked, stranding each old key as the engine minted a successor. The SDK adds
  none of its own packages to the list, so an application that publishes
  packages mints them with `mls_generate_key_package` first. Marking an
  unknown or already used id does nothing, and marking takes the MLS manager's
  write lock, so a concurrent push can no longer erase it. A source guard refuses any new
  record-only delete of a key package
  ([ADR 0012](docs/adr/0012-one-key-package-per-peer.md#a-package-the-application-publishes-keeps-its-record),
  [#367](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/367)).

- **React Native key packages carry their timestamps and synced flag.** The
  wrappers read `createdAt` and `isSynced` from a native record that has only
  ever sent `createdAtMs`, `expiresAtMs` and `synced`, so both were always
  `undefined` in JavaScript. All three key package methods now go through one
  mapper, and `MlsKeyPackage` gains `expiresAt`, the moment a key server
  should drop its copy.

- **`import offline_protocol_sdk` works on Windows.** `pyproject.toml` has
  never installed `bless` on Windows, where it has no backend, and the
  package imported it on the way in, so the import failed on every Windows
  install. The import is guarded; `BlePeripheral.is_available()` is False
  there, as it was documented to be, and everything else is unchanged. Found
  by the first run of the Python suite against a Windows wheel.

- **A lost first key package no longer leaves a pair without a session
  forever.**
  The engine pushed its key package to a peer once and then treated the
  peer as exchanged with, so when that one frame was lost nothing ever sent
  it again. Two peers that dial each other at once lose both pushes when the
  second stream supersedes the first and its late frames are dropped, which
  is what a device test saw in about one fresh first contact in five: the
  stream carried frames, no session formed, and messages queued until they
  failed. A push that has produced no session after 30 seconds is now
  repeated on the next discovery of the peer, or on the reconciliation tick
  while a message waits for that peer. It is the same package, so no key
  material is minted, and the interval is a floor that doubles with each
  repeat up to 10 minutes, so that discovery on every inbound body cannot
  turn into a stream of key packages. A repeat that fails to send waits out
  the window but does not double it, and a blocked peer is never repeated
  to. A lost reset push
  (a re-key or an unblock) is not covered: the repeat carries no reset, so a
  peer that kept its session keeps it
  ([session lifecycle](docs/state-machines/session-lifecycle.md#a-push-that-produced-no-session-is-pushed-again)).

- **Closing the file stores no longer loses unflushed document edits.** An
  edit waits in memory until a flush, and the engine flushes what is left
  when it is dropped. With the built-in file stores that drop runs after
  `close_file_stores()` has closed them and every write is refused, so a
  clean shutdown (Python `ProtocolManager.close()`, and the local API
  service on `SIGTERM`) dropped every edit made since the application last
  called `flush`. `close_file_stores()` now flushes the documents first.

- **The local API service runs its reviewed hardening again.** The squash
  merge of the client examples carried a pre-review copy of the reference
  server and replaced the reviewed one, so `main` ran engine calls on the
  event loop again (a media send stalled every client and the run loop),
  chmod'ed and unlinked whatever sat at `--socket`, kept every issued
  message id for the life of the process, closed the connection on a
  malformed request instead of answering it, and accepted a misspelled
  `denied` entry as denying nothing. The reviewed server, its twelve tests
  and its bridge-contract rows are restored.

- **The local API service starts the gateway client.** With `reticulum`
  enabled, `ProtocolManager` builds the gateway client and leaves starting it
  to its owner, and the service never did, so the slot was a dead carrier.
  The service now starts a configured client with the engine, and
  `offline-protocol-service --gateway HOST:PORT` names the daemon (default
  `localhost:4242`).

- **The iOS config readers read `0` and `1` as numbers.** The Foundation-only
  readers behind `meshRelay` and `custody` excluded JSON booleans with an
  `is Bool` test that Swift also answers true for the numbers 0 and 1, so a
  `fanout: 1`, an `activityIdleWindows: 1` or a `jitterMinMs: 0` written from
  React Native reached the core as unset and the dial silently stayed at its
  default. Both readers now exclude booleans by their CoreFoundation type.

- **The storage conformance suite no longer deletes a merging backend's
  records.** `runStorageConformance` cleaned up its probe records by listing
  a probe key type and deleting what it listed, before any check had run.
  On a backend that merges key types, or lists them by the tail of the name,
  that listing names real records too, and the cleanup deleted them. The
  suite now checks key-type isolation first, with point writes and point
  deletes only, and a failure ends the run there with that one check
  reported. A run that passes it still has thirteen checks, under the same
  names.
- **Relay timestamps parse on Android 7.** The Android bridge parsed them
  with `java.time.Instant`, which exists from Android 8 (API 26), while the
  SDK supports Android 7 (API 24). On Android 7 the call threw
  `NoClassDefFoundError`, which the surrounding `catch (e: Exception)` does
  not catch, for a legacy relay message and an ISO-8601 last-seen timestamp.
  An application that enables core library desugaring was not affected. The
  timestamp is now parsed without `java.time`, as RFC 3339, and gives the
  answers `Instant` gave for every timestamp a relay sends. At the edges it
  is stricter than `Instant`: hour 24, a fraction with no digits, lowercase
  `t` or `z` and a leap second are refused. A Rust guard refuses a new
  `java.time` call in the bridge. Not run on an Android 7 device.
- **An opted-in mesh wake with no wake service stops at once.** A sticky
  restart that wakes JavaScript started the wake service by name, and
  `startService` returns null rather than throwing when no such service is
  declared. The keep-alive then held "Mesh Active" over no mesh until the
  wake watchdog fired. It now checks that the service resolves, and stops as
  it does without the opt-in. A React Native application declares the
  service, so this reached only the native Android library.
- **The iOS deployment target has one reader.** The release build and the
  Swift package read the podspec with two parsers that disagreed on a bare
  major version, a trailing dot and a second declaration. There is one now,
  and it refuses all three. The Swift package's CI job runs the release's
  deployment-target gate on the library it builds, on every pull request.
- **An Android application that minifies can build against the SDK.** Tink,
  which `androidx.security:security-crypto` brings for the MLS store, refers
  to Error Prone's annotation classes and does not ship them, and R8 stops a
  release build on a class it cannot find. The SDK's consumer rules now tell
  R8 to disregard them. A React Native application was affected only if
  nothing else in it brought the annotations.
- **A stopped Python `ProtocolManager` is freed.** The core kept every
  callback the manager registered, and those reached the manager again
  through Rust, where the collector cannot see the cycle, so a stopped
  manager lived as long as the process. `stop()` now replaces all five
  callbacks with inert ones, including one the application registered
  itself for Nostr or Reticulum, so the event handler receives nothing after
  `stop()`. A `stop()` that was cancelled or whose engine stop raised can be
  called again to finish, including one cancelled inside a transport's own
  stop: each transport's `stop()` now resumes from the stopping state
  instead of returning. Overlapping `stop()` calls run one teardown, in
  order, and the BLE peripheral no longer swallows a cancel of its `stop()`.
  `stop()` always takes one loop turn, once the process loop is cancelled:
  a caller retrying after a cancelled `stop()` is inside the task step that
  threw the cancellation, which holds the manager through the exception's
  traceback until the task next suspends, and on Python 3.13 and later
  nothing else in `stop()` is certain to suspend. A teardown also finishes
  on a loop whose default executor was shut down, by running its blocking
  steps in place. A teardown step that raised (a telemetry disable, the
  engine's stop) no longer keeps the manager alive through the logged
  exception's traceback, which Python 3.10 freed only when the collector ran;
  the Python Bindings job now runs the suite on 3.10 as well.
  `__del__` no longer raises on a manager whose constructor raised.
- **Freeing a Python core object can no longer hang the process.** The
  generated bindings guard their callback handle map with a plain lock,
  taken on every callback lookup and removal. The collector can run a
  finalizer inside that critical section, a core object's drop releases
  the callbacks it holds, and each release asked for the same lock on the
  same thread, which then waited for itself. It needed a core object in a
  reference cycle (an event handler that is a method of the manager's
  owner is enough) and a collection at the wrong bytecode, and it could
  not happen while stopped managers were never freed at all. The package
  now gives the handle map a re-entrant lock when it is imported.
- **A protocol-state record key the store reports as corrupt is regenerated.**
  The engine regenerated a record key of the wrong length but treated a
  `CorruptedData` error as a failed load, so a store that seals its records
  and never deletes on a read disabled sensitive persistence on every launch
  for good. `CorruptedData` is reserved for permanent losses, so it now takes
  the wrong-length path: a fresh key, and the records sealed under the old
  one settled as failed on restore. A transient `LoadFailed` still leaves
  the key alone. The rule is now written on `MlsStorage::load` and on the
  secure provider in the UDL and the integration guide, not only on the
  protocol-state provider: a custom secure store that reports a transient
  fault as `CorruptedData` loses every sealed protocol-state record.
- **Python `SecureStorage` warns on a host without a secret service.** Its
  warning about a backend that cannot hold MLS keys checked only the class name,
  and keyring's failing and null backends are both classes named `Keyring`, so a
  headless Linux host or container (which gets `keyring.backends.fail.Keyring`)
  was never warned. The check now also recognises those modules, and judges the
  first backend inside keyring's `ChainerBackend` (the one writes go to) rather
  than the chainer itself, which no check matched. The warning names the
  backend in full (`keyring.backends.fail.Keyring`) and points a headless host
  at the built-in file stores (`store_key`). Log output only; storage behaviour
  is unchanged.
- **Frames a bridge builds for the core carried a fixed application id.** The
  iOS, Android and Python bridges stamped `"offline-messenger"` on every relay
  answer, relay group frame and legacy plain-text DM they rebuilt for the
  core, whatever `appId` the app had configured. They now stamp the
  configured id.
- **The mobile peer-stream managers carry traffic.** Android's Wi-Fi Direct
  manager and iOS's peer-stream manager used to drop every inbound frame and
  announce no peer, because nothing on the wire said who a peer was. Both now
  exchange the identity preamble from
  [the stream chapter](docs/spec/stream-framing.md): each side sends its
  assertion first, checks the peer's with `verifyIdentityAssertion`, and
  announces the peer only under the address it proved. A peer that sends
  anything else first, or nothing for ten seconds, is disconnected unannounced.
  One peer is announced per address, and of two connections for the same
  address one is closed without a loss event (on Android the newer is kept,
  on iOS the one the lower address opened; see Changed). A peer is reported
  lost exactly once, and no message is delivered after that report.
- **Android Wi-Fi Direct framing faults.** The reader refused a message of
  exactly 1 MiB, and after refusing a length it read the skipped body as the
  next length, so every later frame on that socket was misread. It now
  accepts the ceiling and closes the socket on any refused frame. Two writes
  to one socket could also run at once and interleave. Each socket now has
  one ordered writer, so a peer that stops reading stalls only itself.
- **Android Wi-Fi Direct no longer broadcasts a direct message.** A message
  for a peer with no open socket went to every socket in the group. It is now
  dropped, and the core's acknowledgement and retry cover it. A client also
  opened another socket to its group owner on every group change; it now
  keeps one and reconnects it while the group lasts.
- **Android Wi-Fi Direct sends at once, not on a two-second poll.** The
  module registered the transport's send callback at `create` only if a
  Wi-Fi Direct manager already existed, and the manager is built later, by
  `enableTransport`, so it never was. Every message and every acknowledgement
  waited for the manager's 2s fallback poll: on two phones a delivery took a
  median of about three seconds (0.3 to 8), against about 85 ms with the
  callback. It is now registered whenever the config enables the slot.
- **A neighbour is reported lost once, and only when nothing reaches it.**
  Android reports a Bluetooth peer lost once per stale address it gives up
  on, so one departure arrived as two `neighbor_lost`. And a peer that left
  the Wi-Fi Direct group but was still linked over Bluetooth LE (or the
  reverse) was reported lost and cleared from core discovery tracking, so an
  app dropped a neighbour it could still reach. `neighbor_lost` now fires
  when the last mesh link to a peer ends. Bluetooth reported unavailable
  (the radio switched off, or the transport stopped) ends every Bluetooth
  link at once, since Android delivers no per-link disconnect then. The
  Wi-Fi Direct layer going down does the same for its links: iOS takes it
  down when the app backgrounds, before the OS ends the streams, and each
  stream's own late end is no longer a second report.
- **`transport_switched` to Wi-Fi Direct means a peer is connected.** It
  fired when the stream layer came up, which a platform manager does at start
  with or without a peer, saying "Connected to WiFi Direct peer group" while
  every send went over Bluetooth LE. It now fires when the first Wi-Fi Direct
  link is usable and, to `None`, when the last one is not, once per edge: a
  layer going down with several links switches once, and links proved while
  the layer is down switch when it comes up.
- **Android Wi-Fi Direct comes back when Wi-Fi does.** Turning Wi-Fi P2P off
  reported the slot down to the core; turning it back on reported nothing,
  restarted no discovery, and (with group formation on) left the framework's
  dropped service request and record unregistered, so every later discovery
  failed with `NO_SERVICE_REQUESTS`. An app started with Wi-Fi off never got
  Wi-Fi Direct at all. The manager now reports the slot up again when P2P
  returns, restarts discovery and re-registers what formation needs, and
  re-registers the request on that error too.
- **Android rejoins a Wi-Fi Direct group after the app restarts.** The group
  belongs to the system and outlives the process, but since Android 10 the
  connection broadcast is not sticky, so a manager that started inside an
  existing group never heard of it: no listener as owner, no dial as client,
  and the peer stayed unreachable until someone removed the group in the
  system settings. The manager now asks for the group once it is running and
  joins it.
- **React Native starts the peer-stream slot when the config enables it.**
  `start()` brought up internet, Nostr and Reticulum from their config
  sections but not `transports.wifiDirect`, so `wifiDirect: { enabled: true }`,
  the documented way to turn it on, left it off for the whole session with
  nothing logged. It is now enabled after the core starts, and a failure is
  logged rather than thrown, as for the other transports. On iOS this starts
  the Network framework peer stream, so an app that set `enabled: true` there
  now gets the Local Network prompt on first start.
- **The Android module declares the Wi-Fi Direct permissions.** It declared
  the Bluetooth ones but not `ACCESS_WIFI_STATE`, `CHANGE_WIFI_STATE` or
  `NEARBY_WIFI_DEVICES`, so an app that followed the guide reported "WiFi P2P
  is not available on this device" on Android 13 and later. They are declared
  now, `NEARBY_WIFI_DEVICES` with `neverForLocation`, and on Android 13 and
  later the transport no longer requires a location grant. Android 12 and
  lower still gate peer discovery on `ACCESS_FINE_LOCATION`, which the app
  declares and requests. The `neverForLocation` flag merges into every app
  that adds the library, including one that declares the permission itself
  without it; an app that derives location from Wi-Fi replaces the
  declaration with `tools:node="replace"`, and the README says how. The
  library's release check now refuses an AAR whose declaration lost the flag.
- **The Bluetooth LE centrals use the strict verifier.** iOS and Android
  checked a peer's identity with the permissive `verifySignature`, which
  accepts some forged assertions the strict check refuses. They now call
  `verifyIdentityAssertion`, the same check the Python central and both
  peer-stream managers use.
- **A lost last document change of a burst comes back in seconds.** A
  change that arrives after one lost in transit is held and the gap asked for
  at once, but the last change of a burst has nothing after it, so a peer that
  missed it waited for the acknowledgement retry: 10 seconds at the earliest,
  longer under backoff, and never once the retry budget was spent. Three
  seconds after a device's local flushes to a space go quiet, it now sends one
  version offer naming the documents it flushed, and a replica that is behind
  asks for what it lacks. Only a local flush arms it, so it cannot echo or
  chain. Each one
  draws a full version list back from the 1:1 peer or from every group member.
- **iOS: a burst of Bluetooth LE traffic no longer tears messages.** The
  fragment drain pulled the whole backlog out of the core into a bounded
  per-peer queue faster than CoreBluetooth sent it, and the queue then evicted
  its oldest fragments, leaving slices that reassemble into garbage. The drain
  now stops pulling once a peer's queue is three quarters full, on both the
  central write path and the peripheral NOTIFY path, and resumes when the
  queue drains or after one second.
- **The iOS library is built for the pod's deployment target.** The C and
  assembly objects inside it, from `ring` and `oslog`, were stamped for the
  newest iOS the release machine's Xcode knew, so an application linking the
  pod got one linker warning for each of them. The build now reads the pod's
  target, iOS 13.0, hands it to every compiler, and refuses to package an
  archive holding an object built for anything newer. The arm64 simulator
  slice is held to 14.0, the oldest system that simulator has. The SDK's own
  Rust code is compiled for the pod's target as well, where it was compiled
  for the Rust target's floor of iOS 10.

### Changed

- **Dedup defaults grow to 5000 ids and 7 days.** `maxTrackedMessages` goes
  from 2000 to 5000 and `retentionTimeSecs` from 86400 to 604800, on every
  binding and in both React Native fallbacks. Up to 5000 ids are persisted,
  about 365 KB per write. At the cap, this device's own send marks are
  evicted before inbound ids, which have to last the week a sender may
  replay them in. A config that sets `retentionTimeSecs: 86400`
  keeps the old window and the repeated re-sends; see
  [UPGRADING §21](docs/UPGRADING.md#21-delivery-state-survives-a-restart-and-two-defaults-grow-v0260).

- **iOS: an overflowing outbound fragment queue is discarded whole.** Both
  outbound queues, central write and peripheral NOTIFY, used to drop their
  oldest fragments when full. They now discard the whole per-peer queue, as
  Android and the inbound side already did, because a partial cut splits a
  message. The messages lost are re-sent by the acknowledgement retry or by
  data-sync anti-entropy.

- **Documents replicate only when flushed.** This was always true and is now
  documented: an edit changes the open document in memory, and nothing is
  stored or sent to peers until `flush()` or `flushAll()` runs. Flush on a
  short throttle while the user edits, at most about twice a second, and once
  more when editing stops.

- **iOS: the peer-stream slot runs on Network framework, not
  MultipeerConnectivity.** The `wifiDirect` transport on iOS now opens TCP
  streams with `NWListener`, `NWBrowser` and `NWConnection`, over the local
  network or, with no shared network, over AWDL, so two iPhones still reach
  each other with no access point. It advertises and browses
  `_offlineprotocol._tcp` with `txtvers=1` and `addr`, the record the Python
  `PeerStreamManager` uses, so an iPhone and a host on one LAN now find and
  talk to each other. Breaking for apps: an iPhone on this release does not
  see one on 0.27 or earlier over this slot; `NSBonjourServices` needs only
  `_offlineprotocol._tcp` (the `_udp` entry can go), and
  `NSLocalNetworkUsageDescription` is still required. A denied local-network
  permission is now reported as an `error` diagnostic instead of failing
  silently. The seven-peer cap Multipeer imposed is gone; at most sixteen
  streams are open, twelve of them inbound and four from any one address. The
  hop is plain TCP, as on Android and Python, where Multipeer encrypted it;
  payloads are sealed either way. Both ends of a pair now dial, and of two
  streams for one address both keep the one the lower address opened, the
  Python manager's rule
  ([ADR 0027](docs/adr/0027-ios-peer-streams-ride-network-framework.md)).
  The podspec links `Network` in place of `MultipeerConnectivity`. This
  replaces the unreleased Multipeer service-type change, which never
  shipped.

- **Python: `InternetManager` requires `app_id`.** It is now a keyword-only
  argument with no default, because the default was the fixed id above.
  `ProtocolManager` already passes it, so only code that builds an
  `InternetManager` directly needs `app_id=`.

- **The backbone is a gateway property.** The gateway contract's backbone
  section is restated as what any backbone owes a gateway (gateway-to-gateway
  presence, arbitrary-size framing, a provisioned peer list, a declared rate
  class), with Reticulum as the reference backbone in a subsection of its
  own. A gateway advertises each backbone as a `backbone_<kind>_v1`
  capability token; a client ignores a kind it does not know, a gateway may
  advertise none, and the token is advisory and never a routing input. A
  device attaches to one gateway daemon at a time, because every gateway
  answer is recorded against the one daemon carrier. No frame, verb or
  token spelling changes: `backbone_reticulum_v1` is the family's first
  member. The `reticulum_*` entry points keep their names and their doc
  comments now say what they name, the gateway daemon carrier after its
  reference backbone. [ADR 0026](docs/adr/0026-the-backbone-is-a-gateway-property.md)
  records the decision.

## [0.27.0] — 2026-09-25

> **Replicated documents can be removed, narrowed, and carry their bytes
> inside a group.** `removeDoc` and `removeSpace` remove a document from every
> replica instead of having the next exchange refill it, `setInterest` lets a
> device replicate part of a space, and `fetchAttachmentFrom` fetches the bytes
> behind an attachment reference from one group member under the group key.
> Each is negotiated as an appended `data_versions` entry, so a peer on an
> older build keeps working and refuses what it does not understand.
> [§22](docs/UPGRADING.md#22-documents-can-be-removed-from-every-replica-v0270)
> through [§24](docs/UPGRADING.md#24-attachment-bytes-inside-a-group-v0270)
> cover them.
>
> **The peer-stream transport is registered, and a Python host can use it.**
> The `wifi_direct` slot now has a transport behind it, specified in
> `docs/spec/stream-framing.md`, and Python ships a manager that carries it
> over a LAN or a routed mesh. The bundled iOS and Android managers do not
> speak the framing yet, so on a phone the slot announces no peer, refuses
> every recipient, and nothing changes.
>
> **Relay group traffic reaches phones again.** On iOS and Android with
> encryption on, the relay's answers about groups were addressed to the
> profile name and forwarded away unprocessed. They now arrive, so a group
> registered with the relay sends one frame instead of a copy per member,
> group messages other members broadcast through the relay are received, and
> the group events built on those answers fire for the first time.
> [§25](docs/UPGRADING.md#25-behaviour-that-changes-without-a-compile-error-v0270)
> lists this and the other changes that compile fine and still behave
> differently.
>
> **Breaking, narrowly:** the `offline-protocol-transport` crate drops
> `WIFI_DIRECT_MAX_PAYLOAD_SIZE`, and Python's `BlePeripheral` drops its
> `identity_json` argument. No UDL, Swift, Kotlin or TypeScript signature is
> removed.

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
  every stream carries TCP keepalive and an idle dead peer's stream ends
  within about half a minute. Each stream writes from its own queue,
  bounded at 4 MiB while a frame is in flight (a body over it goes back to
  the retry path), and a peer that stops taking bytes once the buffers
  between the two hosts are full has its stream aborted and reported lost
  after thirty seconds without progress (`write_timeout`), without holding
  any other peer's traffic; keepalive alone would not end it, since a
  stream with data in flight is not idle. Until those buffers fill, on the
  order of a megabyte on a default host, a peer that is alive but not
  reading stays announced, and the core's acknowledgements are what notice
  it. A slow link that is moving is never cut. The preamble deadline covers
  the whole first frame,
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

### Documentation

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

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
