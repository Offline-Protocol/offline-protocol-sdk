# Changelog

All notable changes to the Offline Protocol SDK are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog covers
everything after the **v0.7.1** release.

This file holds unreleased changes and the current release. Older releases are
archived by series under [docs/changelog/](docs/changelog/); see the
[archive index](docs/changelog/README.md).

Questions about a release, or about upgrading to one? Ask in the
[developer Discord](https://discord.gg/9TQGrxaPJk).

## [Unreleased]

### Added

- **A reference gateway daemon, and with it LoRa**
  ([ADR 0040](docs/adr/0040-a-reference-gateway-ships-with-the-sdk.md)).
  The `reticulum` transport's device half has spoken the
  [gateway contract](docs/spec/gateway-contract.md) since 0.25, but nothing in
  the SDK answered it, so the transport never became available.
  [`gateway/`](gateway/README.md) is the other half, in Python: devices attach
  over local IP with a proof it checks through
  `verify_gateway_address_proof`, and every submission gets exactly one
  verdict. With a `[reticulum]` section it also carries frames and presence to
  provisioned peer gateways over Reticulum, which is how an RNode's LoRa radio
  reaches the SDK. No device-side change is needed: enable `reticulum` and
  point its daemon address at the gateway. Two real engines pair over MLS and
  exchange a message across two daemon processes joined by a Reticulum
  backbone in its tests; it has not yet been run over radios. Expect a first
  contact between two devices to cost about 20 backbone frames each: by
  arithmetic, a few minutes on the example radio (SF8, 10 % airtime) and
  hours at 1 %. A new gateway is unreachable while Reticulum mints its
  one-time announce stamp.
- **A gateway daemon can verify a device's attach proof without building
  it: `verify_gateway_address_proof(address, public_key, signature,
  challenge)`**, instance-less, in every binding and over the C API's
  instance-less call. It is the gateway half of
  `gateway_address_declaration`: it checks the signature over the one
  canonical payload ([the gateway contract](docs/spec/gateway-contract.md#attach))
  and that the key derives to the claimed address, and returns that address
  or throws. A daemon still owns the third check, that the challenge is the
  one it minted for this connection and has not been accepted before.
- **A browser page can dial a native app on the same Wi-Fi network, or on
  the phone's own hotspot, from one code the app shows: WebRTC Direct**
  ([ADR 0039](docs/adr/0039-a-browser-dials-a-native-device-from-a-code.md)).
  No internet, server or relay is involved, and Safari, Firefox and Chromium
  all dial. The app opens a listener with `openWebrtcDirect()` (React
  Native; `closeWebrtcDirect()` closes it) or `open_webrtc_direct(host,
  port=0)` (Python's `PeerStreamManager`; `close_webrtc_direct()`), and
  shows the code it returns as a QR code or text behind `#` in the page's
  URL. The page dials it with `carrier.dial(code)`, after reading it with
  `parseDirectCode` ([web bridge W16](docs/bridges/web.md#w16-a-page-dials-a-native-device-from-the-code-it-shows)).
  The stream fills the peer-stream slot like any other: the same preamble
  proves the address, and MLS seals the content. The page never edits its
  own session description, so the dial keeps working when libwebrtc stops
  accepting the edit libp2p's first WebRTC Direct relies on. The code
  carries the listener's ICE password and admits whoever holds it for as
  long as the listener is open
  ([R35](docs/security/threat-model.md#r35-a-dial-code-admits-whoever-holds-it-to-an-open-listener)),
  so show it only to the people meant to dial, never log it, and close the
  listener when done; on iOS it lives only while the app is in the
  foreground. When the bridge closes the listener itself (its network or
  hotspot went, the app left the foreground, the transport paused or
  stopped, its socket failed), React Native emits one `webrtc_direct_closed`
  event with the reason, so the app can stop showing a dead code. Opening
  again replaces the listener only once the new one is open, so an open
  that fails leaves the old code working. Custom
  platforms drive the sans-I/O listener through the FFI's
  `webrtc_direct_*` methods, behind the `webrtc-direct` feature, which the
  iOS, Android, desktop and Python release builds turn on; a library built
  without it refuses `webrtc_direct_open`. On two phones, Safari linked to
  the Android app in 56 to 209 ms and delivered 256 KiB in 1.7 to 1.8 s, on
  the home Wi-Fi and on the Android phone's hotspot with no internet alike;
  Chromium, Firefox and WebKit on a laptop linked to the iPhone app in 167
  to 326 ms ([device runs](docs/bridges/web-devices.md#webrtc-direct)). It adds about
  1.54 MB to the iOS library and 2.47 MB to the Android one.

### Changed

- **The mesh `appId` is now `appLabel`** (`app_label` in Rust, Python, Ruby
  and the C API), so it is no longer mistaken for the App ID (`app_…`) the
  developer portal issues for telemetry. Only the name changed: the value
  still selects the storage namespace, is stamped on every frame as the wire
  field `app_id` and derives the Bluetooth app tag, so a shipped app keeps
  its value and old and new builds keep talking. React Native, the browser,
  Flutter and Python still take `appId` (`app_id=`) with a deprecation
  warning, and refuse both names set to different values; Rust keeps
  deprecated `app_id` send options and a `ProtocolConfig::app_id()` getter;
  Swift and Kotlin keep a deprecated `appId` property on the three records,
  and Swift its `appId:` helper overloads; the C API still reads the JSON key
  `app_id`. **Breaking at compile time:** the generated Swift initializer,
  Kotlin named arguments, the Java `appId()` accessor, C# `AppId` and Ruby
  `app_id:` take the new name. Events carry
  `app_label`, with `app_id` beside it for now. The docs now say plainly
  that the label is public and carries no authority. The portal App ID still
  goes to `enableTelemetry` only, and the SDK never reads it from the label.
  Upgrading: [§33](docs/UPGRADING.md#33-the-mesh-appid-is-now-applabel-unreleased).

## [0.34.0] — 2026-11-19

> **Two devices that meet over Bluetooth can bring up Wi-Fi between them.**
> `requestFastLink` asks a peer in a confirmed session for a network: an
> Android device hosts a Wi-Fi Direct group with fresh credentials sealed in
> the session, an iPhone joins it, and transfers move onto it on their own. A
> 1 MiB file that took 197.7 s over Bluetooth took about 2.5 s on two phones.
>
> **Native iOS and Android apps get the SDK's own storage and can check
> theirs.** `OfflineProtocolReferenceStorage` opens the Keychain or Keystore
> stores React Native uses, `StorageConformance.run` returns a typed report
> for a custom backend, and the Swift package is the one iOS install path. The
> Flutter plugin can ship on pub.dev with prebuilt libraries.
>
> **React Native is harder to integrate wrong.** `forScenario` builds a
> working config from a named preset, `mesh-sdk-doctor` finds known host
> mistakes, and a development build explains slow Wi-Fi Direct.
>
> Upgrading from 0.33.x: read
> [§32](docs/UPGRADING.md#32-behaviour-that-changes-without-a-compile-error-v0340).

### Added

- **Two devices that meet only over Bluetooth can bring up Wi-Fi between
  them: the fast link** ([ADR 0038](docs/adr/0038-a-fast-link-is-offered-inside-the-session.md)).
  `requestFastLink(peer)` (React Native and Flutter; `request_fast_link` over
  the FFI) asks a peer with a confirmed session for a Wi-Fi network: an
  Android device hosts a Wi-Fi Direct group with fresh random credentials,
  sealed to the peer in the session, and an iPhone joins it after a system
  prompt. The stream over it fills the peer-stream slot, so transfers move to
  it on their own: a 1 MiB file that took 197.7 s over Bluetooth took about
  2.5 s on two phones. Progress arrives as `fast_link_changed` events; the
  link ends after 60 s with nothing large to carry, when the iPhone app leaves
  the foreground, or on `closeFastLink`, and ends `superseded` when another
  connection, a shared home network say, reaches the peer before it comes up
  and carries the traffic instead. An iOS app needs the Hotspot
  Configuration entitlement, and an Android app `NEARBY_WIFI_DEVICES` (fine
  location below Android 13); see [mobile permissions](docs/mobile-permissions.md#fast-link).
  `wifiDirect.fastLink` decides how far an app takes part: `any` (the
  default) also hosts for a peer's request and joins an offer it did not ask
  for, `requestedOnly` takes part only in links the app asked for, `off`
  never. Platforms declare what they can do through four new peer-stream
  driver calls, `wifi_direct_set_fast_link_roles` and its siblings; a custom
  platform that declares nothing is never asked to host or join.

- **Native iOS and Android applications get the SDK's built-in storage.**
  `OfflineProtocolReferenceStorage` opens the Keychain or Keystore MLS store and
  the app-container protocol-state store for one `(appId, profile)` account,
  hands both to `initializeMls`, and returns what happened to a store written
  before storage was split per account: a `warning` when this account starts
  from a fresh identity. `wipe` erases one account on sign-out. React Native
  opens and wipes its storage through the same code, so the two cannot drift.
  The iOS and Android integration guides use it in place of providers an
  application had to write itself.
- **Native iOS and Android applications can verify a storage backend.**
  `StorageConformance.run` runs the protocol-state conformance suite against a
  `ProtocolStateStorageProvider` and returns a typed report (`passed`,
  `failures` with each check and its detail, `isGreen`) instead of the JSON
  string `runStorageConformance` returns. Green is the definition of a
  supported backend.
- **Native iOS and Android applications get a lab-only cleartext config.**
  `OfflineProtocolDevelopmentCleartextConfig.make(appId, profile)` builds the
  `ProtocolConfig` React Native's `developmentCleartext` scenario reaches: BLE
  on, every other transport off, encryption and `requireEncryption` off, and
  React Native's pending-queue defaults. Setting `encryptionEnabled` to false
  alone is refused, because `requireEncryption` keeps its `true` default. Never
  ship a build that uses it. `OfflineProtocolEncryptFailedDiagnostics` logs a
  one-shot remedy for an `EncryptFailed` send with MLS not initialized, and the
  `ProtocolEngineHost` debug lifecycle hints for a missing MLS now name this
  config instead of `encryptionEnabled = false`, which validation refuses.
- **Native Android applications that copy the `.so` files themselves can check
  the names.** The generated Kotlin loads `libuniffi_offline_protocol.so`,
  cargo writes `liboffline_protocol_uniffi.so`, and a copy that keeps the cargo
  name fails at the first engine call with `UnsatisfiedLinkError`.
  `scripts/check_manual_android_jni_libs.py` checks a `jniLibs` tree, the
  Android library exposes both names as `OfflineProtocolJniLibrary`, and
  [docs/android-jni-and-imports.md](docs/android-jni-and-imports.md) gives the
  names and which package to import the engine and the bridge types from.
  Applications on the Maven library are unaffected: the AAR already carries the
  loaded name.
- **Native iOS applications have one install path.**
  [docs/ios-native-distribution.md](docs/ios-native-distribution.md) installs
  `OfflineProtocolSDK` from `offline-protocol-swift` (or the release's
  `swift-package` tarball) and gives the call order that starts an engine on
  it. The iOS integration guide no longer lists Rust as a prerequisite for an
  application and moves the hand-built XCFramework under a section for
  building the bridge from source. The README shipped in the Swift package
  now gives the same call order, QUICKSTART installs the package instead of
  building Rust, and [UPGRADING §12.3](docs/UPGRADING.md#123-native-ios-bump-the-swift-package-not-a-hand-built-xcframework)
  covers bumping it, including why an app that already has accounts keeps its
  own storage providers.
- **The Flutter plugin can ship on pub.dev with prebuilt native libraries.**
  `scripts/assemble-flutter-pub-package.sh` builds a self-contained package:
  the plugin's tracked files, the shared Android and iOS bridge sources
  vendored beside it, the staged `jniLibs` and XCFramework, and the version and
  deployment target the Gradle build and the podspec would otherwise read from
  the React Native package. An application adds it with
  `flutter pub add offline_protocol` and needs no Rust toolchain. Each release
  assembles it, evaluates its podspec and dry-runs `dart pub publish`; with
  `FLUTTER_PUBLISH` on, a pushed release tag publishes it through pub.dev
  automated publishing. The first version is uploaded by hand
  ([docs/flutter-distribution.md](docs/flutter-distribution.md)).
- **React Native builds a working config from a named scenario.**
  `forScenario(scenario, overrides)` merges `overrides` onto a preset for
  `localBle` (Bluetooth only), `proximityMesh` (Bluetooth and Wi-Fi Direct),
  `internetRelay` (Bluetooth and the internet relay, set `serverAddress`),
  `nostrMesh` (Bluetooth and Nostr, set `relayUrls`) or
  `developmentCleartext` (lab only, encryption off); `scenarioPreset` returns
  the preset alone. Nothing in the engine's defaults changes, and encryption
  stays required in every preset but the lab one
  ([docs/react-native-config-presets.md](docs/react-native-config-presets.md)).
- **React Native can check an app for known integration mistakes.**
  `npx --no-install mesh-sdk-doctor` scans the app root for Expo without
  `expo-dev-client` (Expo Go cannot load the SDK), a leftover
  `pod 'MeshSdk'` line, the old simulator and device archives or their linker
  hacks, and a hand-copied `liboffline_protocol_uniffi.so` under `jniLibs`.
  When the native module fails to load inside Expo Go, the error now says so
  ([docs/react-native-host-doctor.md](docs/react-native-host-doctor.md)).
- **A React Native development build explains slow Wi-Fi Direct and a second
  SDK app on the phone.** With `devTransportDiagnostics` (on under `__DEV__`,
  off in production unless set), known native `diagnostic` events become a
  one-time `console.warn` that links to
  [troubleshooting](docs/troubleshooting.md). Android reports a Wi-Fi Direct
  group still forming at 1, 2 and 4 minutes.
- **React Native can parse an event's JSON with the same rules as the
  engine.** `parseProtocolEvent`, `tryParseProtocolEvent`,
  `narrowProtocolEvent` and `PROTOCOL_EVENT_TYPES` check an `eventJson`
  string's shape and that its `type` is one the package knows, for logging,
  relay taps and tests that read the raw string
  ([T10](docs/bridges/typescript.md#t10-parsing-opaque-event-json)).
- **The browser binding has a Vite starter.** `bindings/web/starter` is a
  tab-to-tab mesh in a module worker, with the prerequisites and how it
  compares with React Native and Flutter in its README. The binding is still
  experimental and not published.

### Changed

- **A file transfer over a peer stream keeps 512 KiB in flight, not 128 KiB:**
  eight 64 KiB chunks instead of four of 32 KiB. Every peer-stream carrier
  gets it (a fast link, a Wi-Fi Direct group, a LAN, AWDL): with the old
  window, a 1 MiB file over a fast link took 8 s waiting on acknowledgements
  rather than on the link. Bluetooth, internet and Nostr windows are
  unchanged.

### Fixed

- **Flutter on Android now reports a lost MLS identity.** Its engine host
  opened the same stores as React Native but never read the legacy-adoption
  outcome, so an account that started from a fresh identity (the earlier store
  belonged to another account, or this account's claim on it could not be
  recorded) did so with nothing logged. It now logs the same error to logcat
  that Flutter on iOS already logs to the unified log.
- **A Python program no longer panics at interpreter exit.** Dropping the
  engine wrote the Lamport clock through the storage callback even when
  `stop()` had just written the same value, and during interpreter shutdown
  that callback can no longer run, so UniFFI panicked
  (`UnexpectedUniFFICallbackError`). Programs using LAN discovery hit it most,
  which is why it was reported as a missing `[lan]` extra. The drop now writes
  only a value storage does not already hold.
- **`offline-protocol-service` refuses a bad socket path in one line.** A
  socket directory other users can read, a path that is not a socket, and a
  path longer than the platform allows (104 bytes on macOS, 108 on Linux) now
  raise `SocketPathError`, a `ValueError`, before the engine starts, and the
  command exits with `--socket: <message>` and the fix instead of a
  traceback.

## Archived releases

Releases before the current one are archived by minor series. Each file carries
its own release table.

| Series | Releases |
|--------|----------|
| [0.33.x](docs/changelog/0.33.md) | 0.33.0 |
| [0.32.x](docs/changelog/0.32.md) | 0.32.0 |
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
