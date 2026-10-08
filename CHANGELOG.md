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

- **The service starts every transport its configuration enables.**
  `offline-protocol-service` started only the peer stream and the gateway
  client, so a configuration with `ble_enabled` built both Bluetooth LE
  managers and never ran either, and `internet_enabled` had no way to name a
  relay. The server now starts the Bluetooth LE central and peripheral where
  the platform has a backend, and the internet transport once `--relay URL`
  names one (the token from `OFFLINE_PROTOCOL_RELAY_TOKEN`, or the variable
  `--relay-token-env` names). A transport that fails to start is logged and
  left stopped while the others run, and the server fails only when none
  starts: a Bluetooth adapter another process holds no longer takes the LAN
  path down with it.
- **`--lan` finds the other hosts on the LAN.** The peer-stream transport's
  DNS-SD advertising and discovery existed in the library and could not be
  switched on from the command; `--lan` switches on both (it needs the `lan`
  extra and fails at start, naming it, without).

- **The HTTP front.** An application that does not use the SDK can call a
  service on another device with a plain HTTP request to
  `<service>.<device>.offline.protocol.internal`. `offline-protocol-service
  --http HOST:PORT` (with the new `http` extra) starts the front in the
  service's process, as a client of the local API; `offline-protocol-http-front`
  runs it apart. A request and its response travel as the content of an end
  to end encrypted direct message, never on the service request path, whose
  bodies are signed plaintext, and a front acts only on a message the engine
  reports as encrypted, so the provider's callback is told the caller's
  MLS-authenticated address in `X-Offline-Protocol-Sender`. A device is named
  by its address, which is a DNS label as it stands, or by an operator alias,
  never by one learned from the LAN. `PUT /services/<name>` registers a
  callback (kept across restarts, and registered with the engine so signed
  discovery finds it), and `GET /browse/<name>` streams the devices that
  offer a service. Bodies are limited to 128 KiB each way, and a request
  waits 30 seconds by default and at most 300. Off loopback the front
  requires a per-launch token, and on any binding it refuses a request
  carrying `Origin`, so a web page cannot reach it by DNS rebinding. `docs/spec/http-front.md` is the contract;
  the threat model gains R23 (the front trusts every process that can reach
  it) and the Python bridge rules gain P13.

### Changed

- **A peer-stream or relay flag the configuration cannot honour is refused.**
  `--listen` and `--peer` without `wifi_direct_enabled` were ignored, which
  left a service that reached nobody and said nothing; they, `--lan`, and
  `--relay` without `internet_enabled`, now stop the command at start with
  the configuration field named. A `--relay` that is not a `ws://` or
  `wss://` URL with a host is refused too: the internet transport retries a
  failed connect for as long as it runs, so a mis-typed relay was a service
  that reported the relay running and passed `/health`. A variable named
  with `--relay-token-env` and left unset is refused rather than read as
  "no token", and each transport has 30 seconds to start before it counts
  as failed and is stopped.

## [0.28.0] — 2026-10-06

> **The peer-stream slot carries traffic on phones.** The mobile managers
> exchange the identity preamble and announce a peer only under the address
> it proved. iOS runs the slot on Network framework, over the local network
> or AWDL, and finds a Python host on the same LAN. Android forms its Wi-Fi
> Direct group itself with `autoAccept: true`, with no system dialog, and
> `wifiDirect: { enabled: true }` now starts the transport from `start()` on
> both platforms.
>
> **Bluetooth LE holds up where it used to give out.** Two phones in a room
> full of other Bluetooth devices find each other, an iPhone's Bluetooth
> power-cycle no longer strands either end, and Android notices Bluetooth
> switched off or a stack crash and relinks. An Android phone takes a peer's
> first write at once instead of dialling the peer back to learn who it is.
> `neighbor_lost` fires once, and only when nothing nearby reaches the peer;
> on Android, 15 seconds after the last link closes.
>
> **The SDK ships outside React Native.** A release now builds, tests and
> publishes a Swift package, an Android library
> (`com.offlineprotocol:offline-protocol-sdk`) and Python wheels tagged for
> their platform. A host with no platform keystore can use the built-in file
> stores, sealed under a store key, and a Python host can front one engine
> for several local applications through the local API.
>
> **Sessions form where they used to stall.** A lost first key package is
> pushed again, and a key package this device's clock refuses is reported as
> `KEY_PACKAGE_OUTSIDE_VALIDITY_WINDOW` instead of leaving messages queued
> with nothing to say why.
>
> **Messages are not lost on the way.** A frame forwarded through a cluster
> where every device hears every other no longer dies inside it, and a contact
> that never confirms its session no longer fills the outbox with probes until
> the user's own messages are evicted. React Native reports an event lost
> between the native module and JavaScript as a `native_event_gap`
> diagnostic.
>
> **Breaking, narrowly:** Python's `InternetManager` requires `app_id=`, and
> an exhaustive TypeScript `switch` over `SecurityWarningCode` needs the new
> member. At run time, an iPhone on 0.28 does not see one on 0.27 or earlier
> over the peer-stream slot.
> [§26](docs/UPGRADING.md#26-behaviour-that-changes-without-a-compile-error-v0280)
> lists these and the other changes that compile fine and behave differently.

### Added

- **An event lost between the native module and JavaScript is reported**
  (refs [#514](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/514)).
  Both native modules number every event they hand to JavaScript, and the SDK
  emits a `diagnostic` event (`message: "native_event_gap"`, with the missing
  range) when a number is skipped. A bridgeless React Native emit can fail
  without the native side seeing it, and three device-test gaps (a receipt
  with no `message_received`, a delivery with no `message_delivered`) could
  not be placed in the core or the bridge. This does not recover a lost
  event; it says which side lost it.

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
  ones before it. Only a relay verdict backed that off, so a mesh-only link
  never did. An account with ten such contacts stacked about two probes a
  second, and once the outbox hit its 500-entry cap, eviction failed the user's
  own messages with `Outbox capacity exceeded`. Seen on a device test between
  an Android and an iPhone, offline over BLE. Each new probe now supersedes
  the last, quietly and without counting against the carrier, whether the
  periodic scan or the Welcome fast path sent it. Probes are no longer written
  to storage, and those an older build wrote are dropped at restore. A peer
  holds one probe in the outbox and is still probed on the same cadence.

- **A message carried through a dense cluster no longer dies inside it**
  ([#510](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/510)).
  A forwarder that received a second copy of a frame while its own forward
  waited cancelled the forward, on the assumption that a neighbor's
  transmission had covered the same ground. It had not: every carrier hands a
  frame to a few chosen neighbors rather than broadcasting it. In a cluster
  where everyone hears everyone, the one device beside the way out could
  stand down after two copies reached it from neighbors that never chose that
  way, and the frame died with its id suppressed on every member, so the
  sender's retries could not rescue it. A copy now takes only the neighbor
  that sent it off the forward's fan-out, and a forward is dropped only when
  every neighbor it could go to has handed it a copy. The
  `mesh_forwarding::everyone_hearing_everyone_does_not_multiply_the_traffic`
  flake (about 2% of runs) went from 4 failures in 200 runs to none. The
  worst-case cost is unchanged: still at most one fan-out per device, inside
  the same per-second budgets.
- **Android reports a Bluetooth peer lost once no link to it is left**
  ([#513](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/513)).
  When the other phone switched Bluetooth off, its links closed cleanly, and
  this phone kept it a neighbour until the reconnect backoff gave up (over a
  minute in the device test, about two in the worst case) or, for a peer
  reached only through this phone's GATT server, until the transport stopped.
  `neighbor_lost` now fires 15 seconds after the last link to the peer goes
  down, unless one comes back first. Reconnect attempts continue during those
  15 seconds; once the peer is reported lost it returns through discovery and
  is announced again, as after any other loss. A dial still in flight does
  not count as a link.
- **Android Bluetooth messages no longer wait on a dial back to the sender**
  ([#512](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/512)).
  A peer's writes reach this phone's GATT server from the peer's central-role
  address. When nothing had mapped that address yet, the server queued the
  fragments and dialled the address to read the peer's identity, and since
  that address does not advertise, the dial took from seconds to about 40 s
  (11 s at p95 in a 30-round soak, 40 s after a Bluetooth toggle). An Android
  central now writes its identity assertion to a new optional GATT
  characteristic, Hello (`6E400006-…`), before its first message write, and
  the server binds the address it proves with the same verifier the Identity
  read uses. A peer without the characteristic, an iPhone among them, is
  resolved as before. [BLE framing](docs/spec/ble-framing.md) specifies it.

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
- **Two phones in a room full of other Bluetooth devices find each other.**
  The dense-mesh filters on iOS and Android counted every advert in range,
  televisions, earbuds and watches included, as mesh density, and in a
  "dense" mesh they passed over up to 80% of the peers, chosen by a hash of
  the peer's address alone. On two Android phones a metre apart in a house,
  the estimate read 32, and each passed the other over on every advert for
  the fifteen minutes they were watched. Density now counts the distinct
  mesh candidates seen recently, so a few phones never read
  as dense, and a peer passed over in a crowded mesh is reconsidered after a
  minute. Both platforms compute the pass-over the same way. A toggle
  while the app is paused drops the dead links but leaves bringing
  Bluetooth back to the resume.
- **Android: Bluetooth switched off and on, or a Bluetooth stack crash, no
  longer strands the mesh.** The transport polled the adapter once a minute,
  so it noticed Bluetooth going off up to a minute late, came back on its
  recovery ladder (12 to 29 s after Bluetooth returned on two phones), and
  never noticed a stack crash at all: on an Android 13 phone the stack
  restarted in under a second and took this app's GATT server, advertiser
  and pending connect with it, and the other phone could not reach it until
  the app restarted. It now listens for the adapter's state broadcast, which
  reports a crash like a toggle. On the way down it reports each peer lost
  and drops the dead links, which Android delivers no disconnect for and
  which otherwise counted against the connection cap, kept peers mapped to
  old addresses, and held GATT client registrations (one phone held five
  stale ones after a session of toggles). On the way up it rebuilds the
  scan, GATT server and advertising at once. A peer probed while its radio
  was coming back is no longer cached as "not a mesh device" for five
  minutes either.

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

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
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
